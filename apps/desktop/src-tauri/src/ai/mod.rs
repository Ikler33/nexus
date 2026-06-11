//! AI-слой (§4.3, **ADR-005**): раздельные Chat / Embedding провайдеры (разные хосты/модели).
//! Ф1-3 — embedding-провайдер; Ф1-7 — chat-провайдер со стримингом.
//!
//! Весь исходящий HTTP провайдеров идёт через [`crate::net::GuardedClient`] (ADR-005-ext, AC-EGR-6):
//! провайдеры ПРИНИМАЮТ guarded-клиент + feature-тег, своих `reqwest::Client` не строят
//! (`core_client_builder` — приватная деталь `net/`, грep-линт AC-EGR-1).

mod chat;
mod config;
mod embedder;

use std::sync::Arc;

pub use chat::{
    build_chat_messages, build_inline_messages, build_rag_messages, build_web_answer_messages,
    build_web_query_messages, injection_marker, parse_web_query_plan, ChatMessage, ChatProvider,
    InlineMode, OpenAiChatProvider,
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
    /// Отказ политики эгресса (ADR-005-ext, AC-EGR-14): типизированная причина, НЕ reqwest-строка.
    #[error(transparent)]
    Denied(#[from] crate::net::EgressDenied),
}

impl From<crate::net::NetError> for AiError {
    fn from(e: crate::net::NetError) -> Self {
        match e {
            crate::net::NetError::Denied(d) => AiError::Denied(d),
            crate::net::NetError::BadUrl => AiError::Config("некорректный URL эгресса".into()),
            crate::net::NetError::Http(e) => AiError::Http(e.to_string()),
        }
    }
}

pub type AiResult<T> = Result<T, AiError>;

/// Тонкий фасад AI-подсистемы (§4.3, AC-EGR-13): ВСЕ провайдеры vault + политика эгресса одним
/// полем `VaultContext` (вместо четырёх независимых `Arc`). БЕЗ `cloud_fallback`/`guard_first_token`
/// — они приходят отдельным срезом вместе с `EgressFeature::CloudFallback` (план §4.3).
///
/// `policy` — тот же `Arc`, что в `AppState` (ОДИН экземпляр политики на приложение): через него
/// hot-swap chat пересобирает уже-guarded клиент, а будущий UI читает состояние для индикации (E9).
pub struct AIClient {
    /// Chat-провайдер (ADR-005, reasoning ON) — стриминг ответов RAG-чата (Ф1-7).
    /// `None`, если в `local.json` нет `ai.chat`. Независим от embedder.
    pub chat: Option<Arc<dyn ChatProvider>>,
    /// «Быстрый» chat без reasoning (R2) на ОСНОВНОЙ модели (gemma) — для дайджеста: большой
    /// контекст без CoT-паузы. Строится вместе с `chat`.
    pub chat_fast: Option<Arc<dyn ChatProvider>>,
    /// «Утилитарная» мелкая модель (`ai.fast`, напр. Qwen3-4B) — короткие примитивы (inline/судья):
    /// низкая латентность. Если `ai.fast` не задан — вызывающие делают fallback на `chat_fast`.
    pub chat_util: Option<Arc<dyn ChatProvider>>,
    /// Embedding-провайдер — эмбеддинг поисковых запросов (Ф1-6) и чат-RAG (Ф1-8).
    /// `None` синхронно с `VaultContext::vectors` (оба есть или обоих нет). Cold: hot-swap нет —
    /// на нём висит фоновый индексатор, смена требует переоткрытия vault (#11b).
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Политика эгресса ядра — единый экземпляр приложения (см. `AppState::egress_policy`).
    pub policy: Arc<crate::net::EgressPolicy>,
}
