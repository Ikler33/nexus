//! AF_UNIX транспорт коннектора (P0b-2c) — `agentd` хостит [`ConnectAgentHandler`] по локальному сокету.
//!
//! Спека `docs/specs/agent-connect.md`: для локального хостинга **AF_UNIX > TCP** (нет сетевого
//! экспонирования, права ОС на файл сокета, нет порта наружу). Кадрирование = **line-delimited JSON**
//! (одно [`RpcMessage`] на строку, `\n`-терминатор) — простейшее устойчивое framing для JSON-RPC поверх
//! байтстрима; парс-сбой строки НЕ роняет соединение (skip + лог), EOF → `recv`=None.
//!
//! `serve_unix_at` биндит сокет и обслуживает подключения: на КАЖДОЕ — свой [`ConnectAgentHandler`]
//! (свой реестр сессий + свой исходящий конец), затем [`dispatch`]-loop. `ConnectDeps` (провайдер/БД/
//! актуатор-конфиг) ШАРЯТСЯ между подключениями. Unix-only (на Windows тип отсутствует — модуль `cfg`).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use super::handler::{ConnectAgentHandler, ConnectDeps};
use super::{dispatch, RpcMessage, Transport, TransportError};

/// Кап длины одной строки-кадра (анти-OOM): клиент без `\n` не должен раздуть буфер бесконечно. Легит
/// JSON-RPC кадры малы; >1 MiB — аномалия → закрываем соединение. NB: для УДАЛЁННОГО (WS) транспорта с
/// недоверенным клиентом (Фаза P1a) кап обязателен ещё строже — здесь 0600-локальный single-owner.
const MAX_LINE_BYTES: usize = 1 << 20;
/// Кап подряд идущих нераспарсенных/невалидных строк: устойчивость к мусору, но не бесконечный CPU/лог-спам
/// от залипшего клиента → после порога закрываем соединение.
const MAX_CONSECUTIVE_MALFORMED: u32 = 64;
/// Потолок backoff accept-loop при повторных ошибках `accept` (анти-spin при исчерпании fd).
const ACCEPT_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(5);

/// [`Transport`] поверх дуплексного `UnixStream` (line-delimited JSON). Поток расщеплён на read/write
/// половины за отдельными мьютексами: `send` (из dispatch-ответов И из drain-тасков событий) сериализуется
/// на write-половине; `recv` (единственный потребитель — serve-loop соединения) — на read-половине. Так
/// конкурентные `send` не путаются, а единственный `recv` читает строки по очереди (контракт `Transport`).
pub struct AfUnixTransport {
    read: Mutex<BufReader<OwnedReadHalf>>,
    write: Mutex<OwnedWriteHalf>,
}

impl AfUnixTransport {
    /// Оборачивает соединённый `UnixStream` (клиентский или принятый сервером).
    pub fn new(stream: UnixStream) -> Self {
        let (r, w) = stream.into_split();
        Self {
            read: Mutex::new(BufReader::new(r)),
            write: Mutex::new(w),
        }
    }
}

#[async_trait]
impl Transport for AfUnixTransport {
    async fn send(&self, msg: RpcMessage) -> Result<(), TransportError> {
        let line = msg.to_json();
        let mut w = self.write.lock().await;
        // Пишем строку + перевод строки + flush. Любая I/O-ошибка ⇒ пир ушёл ⇒ Closed.
        w.write_all(line.as_bytes())
            .await
            .map_err(|_| TransportError::Closed)?;
        w.write_all(b"\n")
            .await
            .map_err(|_| TransportError::Closed)?;
        w.flush().await.map_err(|_| TransportError::Closed)
    }

    async fn recv(&self) -> Option<RpcMessage> {
        let mut r = self.read.lock().await;
        let mut malformed = 0u32;
        loop {
            // Читаем один кадр до `\n` с КАПОМ длины (анти-OOM): fill_buf/consume вместо read_line,
            // чтобы остановиться на MAX_LINE_BYTES, а не аллоцировать бесконечно на потоке без `\n`.
            let mut buf: Vec<u8> = Vec::new();
            let status = loop {
                let available = match r.fill_buf().await {
                    Ok(c) => c,
                    Err(_) => break LineStatus::Closed,
                };
                if available.is_empty() {
                    break LineStatus::Eof; // EOF (неполная строка без `\n` отбрасывается — протокол требует `\n`)
                }
                if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                    buf.extend_from_slice(&available[..pos]);
                    let advance = pos + 1;
                    r.consume(advance);
                    break LineStatus::Line;
                }
                if buf.len() + available.len() > MAX_LINE_BYTES {
                    break LineStatus::TooLong; // кадр > капа → закрываем соединение
                }
                let advance = available.len();
                buf.extend_from_slice(available);
                r.consume(advance);
            };

            match status {
                LineStatus::Eof | LineStatus::Closed => return None,
                LineStatus::TooLong => {
                    tracing::warn!(target: "agent::connect", cap = MAX_LINE_BYTES, "afunix: кадр превысил кап длины — закрываем соединение");
                    return None;
                }
                LineStatus::Line => {
                    let s = match std::str::from_utf8(&buf) {
                        Ok(s) => s.trim(),
                        Err(_) => {
                            malformed += 1;
                            if malformed > MAX_CONSECUTIVE_MALFORMED {
                                return None;
                            }
                            continue;
                        }
                    };
                    if s.is_empty() {
                        continue; // keep-alive/пустая строка — пропускаем (не считаем за malformed)
                    }
                    match RpcMessage::from_json(s) {
                        Ok(m) => return Some(m),
                        Err(_) => {
                            // Нераспарсенная строка НЕ роняет соединение сразу (устойчивость к мусору), но
                            // поток мусора подряд → закрываем (анти CPU/лог-спам).
                            malformed += 1;
                            tracing::warn!(target: "agent::connect", "afunix: пропуск нераспарсенной строки");
                            if malformed > MAX_CONSECUTIVE_MALFORMED {
                                return None;
                            }
                            continue;
                        }
                    }
                }
            }
        }
    }
}

/// Исход чтения одного кадра в [`AfUnixTransport::recv`].
enum LineStatus {
    /// Прочитана полная строка до `\n` (в `buf`).
    Line,
    /// EOF — пир закрыл write-половину.
    Eof,
    /// I/O-ошибка чтения.
    Closed,
    /// Кадр превысил [`MAX_LINE_BYTES`] без `\n`.
    TooLong,
}

/// Готовит путь под bind: stale-сокет от прошлого запуска снимается, НО только если это реально СОКЕТ.
/// Если по пути лежит обычный файл/каталог (мисконфиг `NEXUS_AGENTD_CONNECT_SOCKET`), молчаливое удаление
/// = потеря данных → ОТКАЗ со внятной ошибкой (чужой файл не трогаем). Нет пути → ок (первый старт).
/// `pub(crate)`: переиспользуется host-side `SandboxRunner` (sandbox/runner.rs) для 3 сокетов прогона —
/// ЕДИНАЯ реализация хардненинга bind (без дублирования non-socket-refusal).
pub(crate) fn prepare_socket_path(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            use std::os::unix::fs::FileTypeExt;
            if meta.file_type().is_socket() {
                std::fs::remove_file(path) // наш stale-сокет — снимаем
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "путь сокета {} указывает на НЕ-сокет — отказ (не удаляем чужой файл)",
                        path.display()
                    ),
                ))
            }
        }
        Err(_) => Ok(()), // пути нет — нормальный первый старт
    }
}

/// Защита-в-глубину: сужает права файла сокета до владельца (0600). Коннектор — ПРИВИЛЕГИРОВАННЫЙ peer
/// (драйвит агента, читает vault через tools, тратит токены), поэтому другой локальный пользователь НЕ
/// должен подключиться (connect требует w-право на файл сокета). Local-first single-owner модель;
/// multi-tenant (auth-слой) — позже. Best-effort: не валим запуск, если chmod не удался (FS без unix-прав).
/// `pub(crate)`: переиспользуется host-side `SandboxRunner` для egress/act/event-сокетов (спека §4.2/§4.3 —
/// per-run сокеты 0600; egress.sock даёт guarded-эгресс прогона, act.sock — host-side гейт записи в vault).
pub(crate) fn harden_socket_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(socket = %path.display(), error = %e, "agent-connect: не удалось сузить права сокета до 0600");
    }
}

/// Клиентское подключение к сокету коннектора (для desktop-коннектора / тестов / `nexus`-CLI).
pub async fn connect_unix(path: impl AsRef<Path>) -> std::io::Result<AfUnixTransport> {
    let stream = UnixStream::connect(path).await?;
    Ok(AfUnixTransport::new(stream))
}

/// Биндит сокет по пути (удаляя stale-файл прошлого запуска) и обслуживает подключения навсегда.
/// **default-OFF на уровне вызывающего** (agentd стартует это лишь при заданном `NEXUS_AGENTD_CONNECT_SOCKET`).
pub async fn serve_unix_at(
    socket_path: impl AsRef<Path>,
    deps: Arc<ConnectDeps>,
) -> std::io::Result<()> {
    let path = socket_path.as_ref();
    prepare_socket_path(path)?;
    let listener = UnixListener::bind(path)?;
    harden_socket_perms(path);
    tracing::info!(socket = %path.display(), "agent-connect: AF_UNIX сервер слушает (mode 0600)");
    serve_unix(listener, deps).await;
    Ok(())
}

/// Accept-loop поверх готового `UnixListener` (отделён от bind — тестируется без файловой системы спека
/// сокет-пути). На каждое подключение — свежий [`ConnectAgentHandler`] (изолированный реестр сессий) +
/// dispatch-loop до закрытия соединения. `deps` (Arc) клонируются в каждое соединение.
pub async fn serve_unix(listener: UnixListener, deps: Arc<ConnectDeps>) {
    let mut backoff = std::time::Duration::from_millis(1);
    loop {
        let stream = match listener.accept().await {
            Ok((s, _addr)) => {
                backoff = std::time::Duration::from_millis(1); // успех → сброс backoff
                s
            }
            Err(e) => {
                // Анти-spin при устойчивой ошибке accept (исчерпание fd и т.п.): экспоненциальный backoff.
                tracing::warn!(target: "agent::connect", error = %e, backoff_ms = backoff.as_millis(), "afunix: accept упал — backoff");
                tokio::time::sleep(backoff).await;
                backoff = std::cmp::min(backoff * 2, ACCEPT_BACKOFF_MAX);
                continue;
            }
        };
        let deps = deps.clone();
        tokio::spawn(async move {
            let transport: Arc<dyn Transport> = Arc::new(AfUnixTransport::new(stream));
            let handler = ConnectAgentHandler::new(deps, transport.clone());
            while let Some(msg) = transport.recv().await {
                dispatch(&handler, msg, transport.as_ref()).await;
            }
            tracing::debug!(target: "agent::connect", "afunix: соединение закрыто");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Round-trip [`RpcMessage`] по реальной паре `UnixStream` в ОБА направления (request + notification),
    /// плюс устойчивость к мусорной строке (пропускается, не роняет recv).
    #[tokio::test]
    async fn afunix_transport_roundtrips_both_directions() {
        let (a, b) = UnixStream::pair().unwrap();
        let ta = AfUnixTransport::new(a);
        let tb = AfUnixTransport::new(b);

        // a → b: request
        let req = RpcMessage::request(1, "initialize", json!({"supportedVersions": ["1.0"]}));
        ta.send(req.clone()).await.unwrap();
        assert_eq!(tb.recv().await.unwrap(), req);

        // b → a: notification
        let note = RpcMessage::notification("agent/event", json!({"type": "final", "text": "ок"}));
        tb.send(note.clone()).await.unwrap();
        assert_eq!(ta.recv().await.unwrap(), note);
    }

    /// EOF (пир закрыл write-половину) → `recv` = None.
    #[tokio::test]
    async fn afunix_recv_none_on_eof() {
        let (a, b) = UnixStream::pair().unwrap();
        let ta = AfUnixTransport::new(a);
        drop(b); // пир ушёл
        assert!(ta.recv().await.is_none());
    }

    /// Анти-OOM: кадр длиннее MAX_LINE_BYTES без `\n` → recv закрывает соединение (None), не раздувает память.
    #[tokio::test]
    async fn afunix_recv_closes_on_oversized_frame() {
        use tokio::io::AsyncWriteExt;
        let (a, mut b) = UnixStream::pair().unwrap();
        let ta = AfUnixTransport::new(a);
        // Пишем > капа байт БЕЗ перевода строки — recv должен упереться в кап и вернуть None.
        let huge = vec![b'x'; MAX_LINE_BYTES + 1024];
        tokio::spawn(async move {
            let _ = b.write_all(&huge).await;
            let _ = b.flush().await;
            // держим b открытым недолго, чтобы recv успел прочитать до капа
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });
        assert!(
            ta.recv().await.is_none(),
            "кадр > капа → recv None (анти-OOM)"
        );
    }

    /// `prepare_socket_path`: НЕ-сокет по пути (мисконфиг) → отказ, чужой файл НЕ удалён.
    #[test]
    fn prepare_socket_path_refuses_non_socket() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("real-file.txt");
        std::fs::write(&file, b"important user data").unwrap();
        let err = prepare_socket_path(&file).expect_err("ожидали отказ на не-сокете");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(file.exists(), "чужой файл НЕ должен быть удалён");
        assert_eq!(std::fs::read(&file).unwrap(), b"important user data");
        // Несуществующий путь → Ok (первый старт).
        assert!(prepare_socket_path(&dir.path().join("nope.sock")).is_ok());
    }

    /// Защита-в-глубину: после bind права файла сокета сужены до 0600 (только владелец подключится).
    #[tokio::test]
    async fn socket_perms_hardened_to_0600() {
        use std::os::unix::fs::PermissionsExt;
        let sock = std::env::temp_dir().join(format!("nexus-perm-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        let _listener = UnixListener::bind(&sock).unwrap();
        harden_socket_perms(&sock);
        let mode = std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_file(&sock);
        assert_eq!(
            mode, 0o600,
            "сокет должен быть owner-only (0600), получили {mode:o}"
        );
    }
}
