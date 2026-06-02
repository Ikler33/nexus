//! AI-слой (§4.3, **ADR-005**): раздельные Chat / Embedding провайдеры (разные хосты/модели).
//! Ф1-3 — embedding-провайдер; Ф1-7 — chat-провайдер со стримингом.

mod chat;
mod config;
mod embedder;

pub use chat::{build_rag_messages, ChatMessage, ChatProvider, OpenAiChatProvider};
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
