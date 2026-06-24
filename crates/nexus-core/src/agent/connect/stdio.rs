//! ACP-1 — [`StdioTransport`]: [`Transport`] поверх СПАВНЕННОГО подпроцесса (внешний ACP-агент, напр.
//! `hermes acp`). Клиент спавнит агента и говорит с ним line-delimited JSON-RPC 2.0 по его stdin/stdout
//! (ровно как ACP-спека: «client spawns the agent subprocess … messages delimited by `\n`»). Framing
//! общий с AF_UNIX ([`super::framing`]). Кросс-платформенно (`tokio::process` есть и на Windows).
//!
//! - `stdin` ← наши `send` (запросы/ответы/нотификации клиента).
//! - `stdout` → наши `recv` (нотификации/запросы/ответы агента).
//! - `stderr` → ДРЕНИРУЕТСЯ в `tracing` отдельной задачей: НЕдренированный piped-stderr дедлокнет агента,
//!   как только его буфер заполнится. ОБЯЗАТЕЛЬНО.
//! - `kill_on_drop(true)`: дроп транспорта/бэкенда → агент-подпроцесс убивается (нет осиротевших процессов).

use std::path::Path;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::framing;
use super::{RpcMessage, Transport, TransportError};

/// [`Transport`] поверх stdin/stdout спавненного подпроцесса. `_child` держит хэндл живым (kill_on_drop).
pub struct StdioTransport {
    read: Mutex<BufReader<ChildStdout>>,
    write: Mutex<ChildStdin>,
    _child: Mutex<Child>,
}

impl StdioTransport {
    /// Спавнит `program args…` с `cwd`, пайпит stdin/stdout/stderr, дренирует stderr в лог. Ошибка спавна
    /// (нет бинаря/прав) → `Err` (вызывающий покажет внятное сообщение, vault не ломается).
    pub async fn spawn(program: &str, args: &[String], cwd: &Path) -> std::io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let err =
            |what: &str| std::io::Error::other(format!("StdioTransport: нет {what} у подпроцесса"));
        let stdin = child.stdin.take().ok_or_else(|| err("stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| err("stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| err("stderr"))?;
        tokio::spawn(drain_stderr(stderr, program.to_string()));
        Ok(Self {
            read: Mutex::new(BufReader::new(stdout)),
            write: Mutex::new(stdin),
            _child: Mutex::new(child),
        })
    }
}

/// Построчно сливает stderr подпроцесса в `tracing` (анти-дедлок piped-stderr + наблюдаемость агента).
async fn drain_stderr(stderr: ChildStderr, agent: String) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::warn!(target: "agent::connect::stdio", %agent, "{line}");
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn send(&self, msg: RpcMessage) -> Result<(), TransportError> {
        let mut w = self.write.lock().await;
        framing::send_frame(&mut *w, msg).await
    }
    async fn recv(&self) -> Option<RpcMessage> {
        let mut r = self.read.lock().await;
        framing::recv_frame(&mut *r, "stdio").await
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;

    // `cat` эхо-агент: читает строку из stdin, пишет обратно в stdout (line-delimited) → roundtrip.
    #[tokio::test]
    async fn stdio_roundtrips_a_frame_over_cat() {
        let t = StdioTransport::spawn("cat", &[], Path::new("/"))
            .await
            .expect("spawn cat");
        let msg = RpcMessage::request(1, "ping", json!({"x":1}));
        t.send(msg.clone()).await.expect("send");
        let got = t.recv().await.expect("recv echoed frame");
        assert_eq!(got, msg);
    }

    // Подпроцесс мгновенно завершается → stdout закрыт → recv = None (не виснет).
    #[tokio::test]
    async fn stdio_recv_none_on_child_eof() {
        let t = StdioTransport::spawn("true", &[], Path::new("/"))
            .await
            .expect("spawn true");
        assert!(t.recv().await.is_none(), "EOF подпроцесса → None");
    }

    // Несуществующий бинарь → spawn возвращает Err (не паника).
    #[tokio::test]
    async fn stdio_spawn_missing_binary_errs() {
        let r = StdioTransport::spawn("definitely-no-such-binary-xyz", &[], Path::new("/")).await;
        assert!(r.is_err(), "нет бинаря → Err");
    }
}
