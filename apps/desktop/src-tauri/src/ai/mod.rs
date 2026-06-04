//! AI-слой (§4.3, **ADR-005**): раздельные Chat / Embedding провайдеры (разные хосты/модели).
//! Ф1-3 — embedding-провайдер; Ф1-7 — chat-провайдер со стримингом.

mod chat;
mod config;
mod embedder;

pub use chat::{
    build_chat_messages, build_rag_messages, injection_marker, ChatMessage, ChatProvider,
    OpenAiChatProvider,
};
pub use config::{AiConfig, ChatConfig, EmbeddingConfig, LocalConfig};
#[cfg(test)]
pub(crate) use embedder::MockEmbedder;
pub use embedder::{default_prefixes, l2_normalize, EmbeddingProvider, OpenAiEmbedder};

use thiserror::Error;

/// Ошибки AI-слоя.
#[derive(Debug, Error)]
pub enum AiError {
    #[error("http: {0}")]
    Http(String),
    #[error("некорректный ответ модели: {0}")]
    BadResponse(String),
    #[error("размерность вектора: ожидалось {expected}, получено {got}")]
    DimMismatch { expected: usize, got: usize },
    #[error("config: {0}")]
    Config(String),
}

pub type AiResult<T> = Result<T, AiError>;

/// Общий конструктор HTTP-клиента ядра для LLM-серверов (chat/embedding): **не следует редиректам**
/// (анти-SSRF, AC-SEC-4 / ревью C5). Скомпрометированный или подменённый эндпоинт не может 30x-редиректом
/// увести запрос ядра на внутренний/metadata-адрес. Таймауты задаёт вызывающий.
///
/// `is_private_host` к ядру намеренно НЕ применяется: chat/embedding-серверы локальные/LAN by design
/// (`127.0.0.1`, `192.168.*`) — блок приватных хостов сломал бы local-first. Различие с plugin
/// `net.fetch` (allowlist + `is_private_host` для произвольного egress) — осознанное (ревью C5/H11).
pub(crate) fn core_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// AC-SEC-4 / ревью C5: core-HTTP-клиент НЕ следует редиректам. Локальный сервер отдаёт 302 на
    /// metadata-адрес; клиент обязан вернуть сам 302, а не пойти по `Location` (иначе — SSRF).
    #[tokio::test]
    async fn core_client_does_not_follow_redirects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let resp = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\n\r\n";
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        let client = core_client_builder().build().unwrap();
        let resp = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("запрос к локальному серверу");
        assert_eq!(
            resp.status().as_u16(),
            302,
            "core-клиент НЕ должен следовать редиректу (анти-SSRF, AC-SEC-4)"
        );
        server.join().unwrap();
    }
}
