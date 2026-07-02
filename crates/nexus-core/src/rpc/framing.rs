//! Общий line-delimited JSON-RPC framing для потоковых транспортов (AF_UNIX и stdio). Вынесен из
//! `afunix.rs`, чтобы `AfUnixTransport` (CONN-2) и `StdioTransport` (ACP-1, `agent::connect::stdio`)
//! НЕ дрейфовали по форматированию/анти-OOM-капам. Спека ACP: «Messages are delimited by newlines
//! (`\n`), and MUST NOT contain embedded newlines» — идентично нашему AF_UNIX-фреймингу, поэтому код
//! общий. R-1: живёт в транспорт-нейтральном [`crate::rpc`]; путь `agent::connect::framing` сохранён
//! pub(crate)-реэкспортом.

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use super::{RpcMessage, TransportError};

/// Кап длины одного кадра (анти-OOM на потоке без `\n`).
pub(crate) const MAX_LINE_BYTES: usize = 1 << 20;
/// Кап подряд идущих нераспарсенных/невалидных строк → после порога закрываем соединение
/// (устойчивость к мусору, но не бесконечный CPU/лог-спам от залипшего пира).
pub(crate) const MAX_CONSECUTIVE_MALFORMED: u32 = 64;

/// Исход чтения одного кадра.
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

/// Пишет одно сообщение как `<json>\n` + flush. Любая I/O-ошибка ⇒ пир ушёл ⇒ [`TransportError::Closed`].
pub(crate) async fn send_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    msg: RpcMessage,
) -> Result<(), TransportError> {
    let line = msg.to_json();
    w.write_all(line.as_bytes())
        .await
        .map_err(|_| TransportError::Closed)?;
    w.write_all(b"\n")
        .await
        .map_err(|_| TransportError::Closed)?;
    w.flush().await.map_err(|_| TransportError::Closed)
}

/// Читает один [`RpcMessage`]-кадр из line-delimited потока. `None` — транспорт закрыт (EOF/ошибка/кап).
/// `label` — для логов («afunix»/«stdio»). Единственный потребитель на эндпоинт (контракт `Transport`).
pub(crate) async fn recv_frame<R: AsyncBufRead + Unpin>(
    r: &mut R,
    label: &str,
) -> Option<RpcMessage> {
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
                tracing::warn!(target: "agent::connect", %label, cap = MAX_LINE_BYTES, "кадр превысил кап длины — закрываем соединение");
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
                        malformed += 1;
                        tracing::warn!(target: "agent::connect", %label, "пропуск нераспарсенной строки");
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
