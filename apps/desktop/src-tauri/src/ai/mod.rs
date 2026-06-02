//! AI-слой (§4.3, **ADR-005**): раздельные Chat / Embedding провайдеры (разные хосты/модели).
//! Ф1-3 — embedding-провайдер; chat-провайдер придёт в Ф1-7.

mod config;
mod embedder;

pub use config::{AiConfig, ChatConfig, EmbeddingConfig, LocalConfig};
pub use embedder::{l2_normalize, EmbeddingProvider, OpenAiEmbedder};
// MockEmbedder (тест-мок) реэкспортируется из `embedder` для кросс-модульных тестов начиная с Ф1-4.

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
