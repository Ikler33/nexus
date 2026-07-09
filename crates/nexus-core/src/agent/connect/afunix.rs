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
//!
//! **Авторизация peer'а (T8).** Контрол-сокет коннектора — второй 0600 AF_UNIX-листенер, названный
//! THREAT_MODEL T8 (`agent-sandbox.md §10.1` / `agent-connect.md §6`). ПОВЕРХ 0600-прав accept-loop
//! применяет [`connector_peer_authorized`]: ожидаемый peer = **ОПЕРАТОР** (uid процесса `agentd`,
//! [`operator_uid`]), а НЕ run_as-uid контейнера (как у per-run sandbox-сокетов, `sandbox/runner.rs`).
//! Linux — fail-closed по `SO_PEERCRED` (переиспользует [`peer_uid`]); не-Linux — perms-only fallback
//! (см. [`connector_peer_authorized`]).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::BufReader;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use super::framing;
use super::handler::{ConnectAgentHandler, ConnectDeps};
use super::{dispatch, RpcError, RpcMessage, Transport, TransportError};

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
        let mut w = self.write.lock().await;
        framing::send_frame(&mut *w, msg).await
    }

    async fn recv(&self) -> Option<RpcMessage> {
        let mut r = self.read.lock().await;
        framing::recv_frame(&mut *r, "afunix").await
    }
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

/// Читает uid пира соединённого AF_UNIX-сокета через `SO_PEERCRED` (Linux). Это **ядро-достоверный**
/// credential: клиент НЕ может его подделать (в отличие от любого прикладного поля внутри RPC-кадра).
/// Host-side `SandboxRunner` использует его, чтобы пустить на per-run сокет (egress/act/event, и будущий
/// exec) ТОЛЬКО спавненный контейнер — peer, бегущий под run_as-uid прогона (спека `agent-sandbox.md`
/// §4.3 инвариант 6 / §10.1 T8: анти-подмена peer'а поверх 0600-сокета + 0700-каталога). **Fail-closed:**
/// сбой `getsockopt` / усечённый credential → `None` (вызывающий ОБЯЗАН дропнуть соединение).
/// `pub(crate)`: тот же контракт применим к будущему control-/exec-сокету.
#[cfg(target_os = "linux")]
pub(crate) fn peer_uid(stream: &UnixStream) -> Option<u32> {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `getsockopt(SOL_SOCKET, SO_PEERCRED)` на соединённом AF_UNIX-fd заполняет `ucred`. Передаём
    // корректно-размерный обнулённый out-буфер и его длину inout; читаем поля ТОЛЬКО при rc==0 И неусечённой
    // длине. `fd` валиден на всё время вызова (заимствование `stream` живёт дольше), syscall не сохраняет fd.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast::<libc::c_void>(),
            &mut len,
        )
    };
    if rc != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        // Отличаем «cred НЕЧИТАЕМ» (аномалия ядра/fd) от «uid НЕ СОВПАЛ» (логируется на call-site): для
        // security-гейта это разные события в аудит-следе. Всё равно fail-closed → вызывающий дропнет.
        tracing::warn!(
            target: "agent::connect",
            rc,
            got_len = len,
            want_len = std::mem::size_of::<libc::ucred>(),
            "afunix: getsockopt(SO_PEERCRED) не прочёл peer-cred — fail-closed (соединение будет отвергнуто)"
        );
        return None;
    }
    Some(cred.uid)
}

/// Не-Linux: `SO_PEERCRED` отсутствует. Песочница — Linux-host-only (`agent-sandbox.md` §9): на иных ОС
/// serve-путь раннера не достигается в проде. Возвращаем `None` (fail-closed — вызывающий дропнет соединение).
#[cfg(not(target_os = "linux"))]
pub(crate) fn peer_uid(_stream: &UnixStream) -> Option<u32> {
    None
}

/// Ожидаемый peer-uid КОНТРОЛ-сокета коннектора = uid САМОГО процесса `agentd` (**ОПЕРАТОР**). Это ключевое
/// отличие от per-run sandbox-сокетов, где ожидается run_as-uid спавненного контейнера (`sandbox/runner.rs`):
/// контрол-сокет драйвит не контейнер, а оператор (тот же uid, что у хостящего процесса). Передаётся в
/// [`serve_unix`]/[`serve_unix_at`] как `expected_uid`. Linux → `Some(getuid())`; не-Linux → `None` (там
/// `SO_PEERCRED` нет → peer-гейт неприменим, fallback на 0600-права, см. [`connector_peer_authorized`]).
/// `pub`: вызывается из `nexus-agentd` (внешний крейт) на call-site `serve_unix_at`.
#[cfg(target_os = "linux")]
pub fn operator_uid() -> Option<u32> {
    // SAFETY: `getuid()` инфаллибелен и без side-effects (POSIX: always succeeds), fd/указателей не берёт.
    Some(unsafe { libc::getuid() })
}

/// Не-Linux: `SO_PEERCRED` недоступен → ожидаемый uid не вычисляем (peer-гейт там перм-онли). См.
/// [`operator_uid`] (Linux) и [`connector_peer_authorized`] (fallback-семантика).
#[cfg(not(target_os = "linux"))]
pub fn operator_uid() -> Option<u32> {
    None
}

/// Авторизует принятое на КОНТРОЛ-сокете коннектора ([`serve_unix`]) соединение. Ожидаемый peer = ОПЕРАТОР
/// ([`operator_uid`]), в отличие от per-run sandbox-сокетов (там run_as-uid контейнера,
/// `sandbox/runner.rs::peer_authorized`).
///
/// **Linux:** fail-closed гейт ПОВЕРХ 0600-прав (`agent-connect.md §6` / `agent-sandbox.md §10.1 T8`):
/// пускаем ТОЛЬКО при достоверном равенстве `peer_uid == expected`. Нечитаемый cred ([`peer_uid`]=`None`),
/// mismatch ИЛИ неизвестный ожидаемый (`expected`=`None`) → отказ — семантика идентична sandbox-`uid_matches`.
#[cfg(target_os = "linux")]
fn connector_peer_authorized(stream: &UnixStream, expected_uid: Option<u32>) -> bool {
    matches!((expected_uid, peer_uid(stream)), (Some(e), Some(a)) if e == a)
}

/// Не-Linux: `SO_PEERCRED` отсутствует ([`peer_uid`]=`None`), а контрол-сокет коннектора —
/// КРОСС-ПЛАТФОРМЕННЫЙ (`#[cfg(unix)]`: dev/CI на macOS, E2E-тест `serve_unix_drives_run_over_socket`).
/// Поэтому peer-uid-гейт структурно неприменим → **fallback на 0600-права** файла сокета (single-owner
/// local-first): соединение пускаем. NB: sandbox так НЕ делает — он Linux-host-only (§9), там `None`
/// фейлится наглухо; коннектору же strict-fail-closed на macOS оборвал бы все соединения.
#[cfg(not(target_os = "linux"))]
fn connector_peer_authorized(_stream: &UnixStream, _expected_uid: Option<u32>) -> bool {
    true
}

/// Клиентское подключение к сокету коннектора (для desktop-коннектора / тестов / `nexus`-CLI).
pub async fn connect_unix(path: impl AsRef<Path>) -> std::io::Result<AfUnixTransport> {
    let stream = UnixStream::connect(path).await?;
    Ok(AfUnixTransport::new(stream))
}

/// Диагностический вердикт пути сокета коннектора ДО `connect` (CONN-4/P0b). ЕДИНАЯ классификация
/// для клиентских потребителей — CLI (`nexus status`), desktop test-connection и lazy-connect
/// бэкенда: отличить «сервис не запущен» (файла нет) от «мисконфиг» (по пути НЕ сокет) до попытки
/// соединения. UX-СООБЩЕНИЕ канон НЕ формирует — тексты контекстно-зависимы (флаг `--socket` у CLI,
/// ключ `ai.connection.socket` у desktop), вызыватель форматирует свой БАЙТ-ПРЕЖНИЙ текст по вердикту.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketDiag {
    /// Путь — сокет ЛИБО метаданные не читаются (ошибка НЕ `NotFound`) → `connect` сам разберётся
    /// (не блокируем диагностикой — прежнее поведение `_ => Ok`).
    Usable,
    /// Путь существует, но это НЕ сокет (вероятный мисконфиг пути сокета).
    NotSocket,
    /// Файла сокета по пути нет (сервис/демон, вероятно, не запущен).
    Missing,
}

/// Классифицирует путь сокета ДО `connect` (см. [`SocketDiag`]). ЕДИНЫЙ источник диагностики
/// «нет файла vs не-сокет» для всех клиентских потребителей коннектора — снят тройной дубль
/// разбора `symlink_metadata` (CLI `cmd_status` / desktop `classify_socket` / desktop backend `connect`).
pub fn classify_socket(path: &Path) -> SocketDiag {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_socket() => SocketDiag::NotSocket,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SocketDiag::Missing,
        _ => SocketDiag::Usable,
    }
}

/// Ошибка клиентской пробы [`probe_initialize`]. Разделяет ОБЩИЕ (байт-идентичные у всех
/// потребителей) диагностические тексты от контекстных, которые форматирует вызыватель.
#[derive(Debug)]
pub enum ProbeError {
    /// Готовое внятное сообщение (не удалось отправить `initialize` / таймаут / закрыто без ответа) —
    /// байт-идентично у CLI и desktop, поэтому его несёт сам канон.
    Message(String),
    /// agentd ответил RPC-ошибкой — вызыватель формирует свой текст из полей (`message`/`code`).
    Rpc(RpcError),
    /// Ответ не является `Response` (протокол-аномалия) — вызыватель форматирует `{other:?}`.
    Unexpected(RpcMessage),
}

/// Клиентская проба agentd: `initialize` по УЖЕ подключённому транспорту → строка версии протокола
/// (`version` из результата, либо `"?"`). ЕДИНАЯ реализация handshake-пробы для CLI (`nexus status`) и
/// desktop test-connection — снят дубль `send → recv(timeout) → parse`. Тексты send-fail/timeout/
/// closed БАЙТ-ИДЕНТИЧНЫ у обоих потребителей (их несёт канон через [`ProbeError::Message`]); connect
/// и формат RPC-ошибки/неожиданного ответа — за вызывателем (тексты различны).
pub async fn probe_initialize<T: Transport + ?Sized>(
    transport: &T,
    timeout: std::time::Duration,
) -> Result<String, ProbeError> {
    transport
        .send(RpcMessage::request(
            1,
            "initialize",
            serde_json::json!({ "supportedVersions": ["1.0"] }),
        ))
        .await
        .map_err(|_| {
            ProbeError::Message("не удалось отправить initialize (сокет закрылся)".to_string())
        })?;
    let resp = tokio::time::timeout(timeout, transport.recv())
        .await
        .map_err(|_| ProbeError::Message("таймаут ответа initialize".to_string()))?
        .ok_or_else(|| ProbeError::Message("сокет закрыт без ответа".to_string()))?;
    match resp {
        RpcMessage::Response { result: Ok(v), .. } => Ok(v
            .get("version")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string()),
        RpcMessage::Response { result: Err(e), .. } => Err(ProbeError::Rpc(e)),
        other => Err(ProbeError::Unexpected(other)),
    }
}

/// Биндит сокет по пути (удаляя stale-файл прошлого запуска) и обслуживает подключения навсегда.
/// **default-OFF на уровне вызывающего** (agentd стартует это лишь при заданном `NEXUS_AGENTD_CONNECT_SOCKET`).
/// `expected_uid` — ожидаемый peer-uid оператора для T8-гейта accept'а (см. [`serve_unix`] / [`operator_uid`]).
pub async fn serve_unix_at(
    socket_path: impl AsRef<Path>,
    deps: Arc<ConnectDeps>,
    expected_uid: Option<u32>,
) -> std::io::Result<()> {
    let path = socket_path.as_ref();
    prepare_socket_path(path)?;
    let listener = UnixListener::bind(path)?;
    harden_socket_perms(path);
    tracing::info!(socket = %path.display(), "agent-connect: AF_UNIX сервер слушает (mode 0600)");
    serve_unix(listener, deps, expected_uid).await;
    Ok(())
}

/// Accept-loop поверх готового `UnixListener` (отделён от bind — тестируется без файловой системы спека
/// сокет-пути). На каждое подключение — свежий [`ConnectAgentHandler`] (изолированный реестр сессий) +
/// dispatch-loop до закрытия соединения. `deps` (Arc) клонируются в каждое соединение.
///
/// `expected_uid` — ожидаемый peer-uid ОПЕРАТОРА (`= uid agentd`, [`operator_uid`]) для T8-гейта
/// ([`connector_peer_authorized`]): соединение, чей `SO_PEERCRED`-uid не совпал (Linux), дропается
/// ПЕРЕД dispatch'ем; на не-Linux — perms-only.
pub async fn serve_unix(listener: UnixListener, deps: Arc<ConnectDeps>, expected_uid: Option<u32>) {
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
        // T8 (agent-connect §6 / agent-sandbox §10.1): defense-in-depth ПОВЕРХ 0600 — пускаем ТОЛЬКО
        // оператора (peer-uid == uid agentd) по SO_PEERCRED. Нечитаемый cred / mismatch / неизвестный
        // ожидаемый → дроп + warn И слушаем дальше (импостор не лишает оператора сервиса — как accept-loop
        // sandbox-раннера). Не-Linux → perms-only. `stream` дропается выходом из scope (FIN пиру).
        if !connector_peer_authorized(&stream, expected_uid) {
            tracing::warn!(target: "agent::connect", "afunix: соединение отвергнуто — peer-uid != uid оператора (SO_PEERCRED, T8)");
            continue;
        }
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
        let huge = vec![b'x'; super::framing::MAX_LINE_BYTES + 1024];
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

    /// CONN-4 канон: [`classify_socket`] различает сокет / НЕ-сокет / отсутствующий путь. Это ЕДИНАЯ
    /// диагностика «нет файла vs не-сокет», через которую роутятся CLI и оба desktop-потребителя.
    #[tokio::test]
    async fn classify_socket_distinguishes_socket_file_missing() {
        let dir = TempDir::new().unwrap();
        // Реальный сокет (bind) → Usable.
        let sock = dir.path().join("real.sock");
        let _listener = UnixListener::bind(&sock).unwrap();
        assert_eq!(classify_socket(&sock), SocketDiag::Usable, "сокет → Usable");
        // Обычный файл → NotSocket (мисконфиг).
        let file = dir.path().join("plain.txt");
        std::fs::write(&file, b"not a socket").unwrap();
        assert_eq!(
            classify_socket(&file),
            SocketDiag::NotSocket,
            "обычный файл → NotSocket"
        );
        // Отсутствующий путь → Missing (сервис не запущен).
        assert_eq!(
            classify_socket(&dir.path().join("gone.sock")),
            SocketDiag::Missing,
            "нет файла → Missing"
        );
    }

    /// CONN-4 канон: [`probe_initialize`] шлёт `initialize` и извлекает `version` из ответа. Мок-пир
    /// на второй половине пары отвечает Response с версией — проба возвращает её строкой.
    #[tokio::test]
    async fn probe_initialize_returns_version_from_response() {
        let (a, b) = UnixStream::pair().unwrap();
        let client = AfUnixTransport::new(a);
        let server = AfUnixTransport::new(b);
        // Мок-agentd: принять initialize-request, ответить версией «1.0».
        let srv = tokio::spawn(async move {
            let msg = server.recv().await.unwrap();
            let id = match msg {
                RpcMessage::Request { id, method, .. } => {
                    assert_eq!(method, "initialize");
                    id
                }
                other => panic!("ждали initialize-Request, получили {other:?}"),
            };
            server
                .send(RpcMessage::Response {
                    id,
                    result: Ok(serde_json::json!({ "version": "1.0" })),
                })
                .await
                .unwrap();
        });
        let ver = probe_initialize(&client, std::time::Duration::from_secs(5))
            .await
            .expect("проба успешна");
        assert_eq!(ver, "1.0", "версия извлечена из ответа");
        srv.await.unwrap();
    }

    /// CONN-4 канон: RPC-ошибка ответа → [`ProbeError::Rpc`] (вызыватель форматирует свой текст).
    #[tokio::test]
    async fn probe_initialize_surfaces_rpc_error() {
        let (a, b) = UnixStream::pair().unwrap();
        let client = AfUnixTransport::new(a);
        let server = AfUnixTransport::new(b);
        let srv = tokio::spawn(async move {
            let id = match server.recv().await.unwrap() {
                RpcMessage::Request { id, .. } => id,
                other => panic!("ждали Request, получили {other:?}"),
            };
            server
                .send(RpcMessage::Response {
                    id,
                    result: Err(RpcError::version_incompatible()),
                })
                .await
                .unwrap();
        });
        match probe_initialize(&client, std::time::Duration::from_secs(5)).await {
            Err(ProbeError::Rpc(e)) => assert_eq!(e, RpcError::version_incompatible()),
            _ => panic!("ждали ProbeError::Rpc, получили результат иного вида"),
        }
        srv.await.unwrap();
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

    /// **Tier-1 (Linux):** T8-гейт контрол-сокета на РЕАЛЬНОЙ паре `UnixListener`↔`UnixStream`. Ожидаемый
    /// peer = ОПЕРАТОР ([`operator_uid`] = `getuid()`). `SO_PEERCRED` читает наш uid → соединение того же
    /// uid АВТОРИЗУЕТСЯ; заведомо-чужой ожидаемый uid (mismatch-ветка БЕЗ привилегий — через неверный
    /// `expected`) и неизвестный ожидаемый (`None`) — ОТВЕРГАЮТСЯ (fail-closed, идентично sandbox-семантике).
    /// Аналог `sandbox/runner.rs::peer_authorized_accepts_same_uid_rejects_mismatch`. Кросс-uid РЕАЛЬНЫМ
    /// вторым пользователем — Tier-2 (нужны привилегии). На не-Linux гейт perms-only → тест Linux-gated.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn connector_peer_authorized_accepts_same_uid_rejects_mismatch() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("connect.sock");
        let listener = UnixListener::bind(&path).unwrap();
        // accept (сервер) и connect (клиент) — оба наш процесс ⇒ один uid (как оператор драйвит коннектор).
        let (accepted, _client) =
            tokio::join!(async { listener.accept().await.unwrap().0 }, async {
                UnixStream::connect(&path).await.unwrap()
            },);
        let me = unsafe { libc::getuid() };
        assert_eq!(
            operator_uid(),
            Some(me),
            "operator_uid() == getuid() (Linux)"
        );
        assert_eq!(peer_uid(&accepted), Some(me), "SO_PEERCRED читает наш uid");
        assert!(
            connector_peer_authorized(&accepted, Some(me)),
            "тот же uid (оператор) → авторизован"
        );
        assert!(
            !connector_peer_authorized(&accepted, Some(me.wrapping_add(1))),
            "чужой ожидаемый uid → отвергнут"
        );
        assert!(
            !connector_peer_authorized(&accepted, None),
            "неизвестный ожидаемый → отвергнут (fail-closed на Linux)"
        );
    }
}
