//! AI-слой (§4.3, **ADR-005**): раздельные Chat / Embedding провайдеры (разные хосты/модели).
//! Ф1-3 — embedding-провайдер; Ф1-7 — chat-провайдер со стримингом.
//!
//! Весь исходящий HTTP провайдеров идёт через [`crate::net::GuardedClient`] (ADR-005-ext, AC-EGR-6):
//! провайдеры ПРИНИМАЮТ guarded-клиент + feature-тег, своих `reqwest::Client` не строят
//! (`core_client_builder` — приватная деталь `net/`, грep-линт AC-EGR-1).

mod chat;
mod config;
mod embedder;
mod tokenizer;
/// Tool-capable chat-провайдер (AGENT-1, I-5): ОТДЕЛЬНЫЙ от `chat` тип — tools не протекают в chat-путь.
pub mod tools;

use std::sync::Arc;

pub use chat::{
    build_agent_memory_block, build_chat_messages, build_episode_block, build_inline_messages,
    build_memory_block, build_pinned_block, build_rag_messages, build_web_answer_messages,
    build_web_query_messages, fence_observation, injection_marker, parse_web_query_plan,
    prepend_memory_block, ChatMessage, ChatProvider, InlineMode, OpenAiChatProvider, ToolCallFn,
    ToolCallMsg, WebQueryPlan, FENCE_MAX_BYTES,
};
pub use config::{AiConfig, ChatConfig, EmbeddingConfig, LocalConfig};
#[cfg(any(test, feature = "test-util"))]
pub use embedder::MockEmbedder;
pub use embedder::{default_prefixes, l2_normalize, EmbeddingProvider, OpenAiEmbedder};
pub use tokenizer::{ContextBudget, HeuristicTokenizer, QwenTokenizer};

use thiserror::Error;

/// Нормализует базовый URL OpenAI-совместимого сервера: снимает хвостовые `/` и опциональный
/// суффикс `/v1`. Иначе пользовательский URL вида `http://host:8080/v1` после добавления
/// канонического пути даёт `…/v1/v1/chat/completions` → 404 (баг 2026-06-11: чат «Доступен» по
/// probe, но реальные вызовы падали из-за двойного `/v1`). Терпим оба варианта ввода.
pub fn api_base(url: &str) -> String {
    let t = url.trim().trim_end_matches('/');
    let t = t.strip_suffix("/v1").unwrap_or(t);
    t.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod api_base_tests {
    use super::api_base;

    #[test]
    fn strips_trailing_slash_and_v1() {
        assert_eq!(api_base("http://h:8080"), "http://h:8080");
        assert_eq!(api_base("http://h:8080/"), "http://h:8080");
        // Главный кейс бага: пользователь вставил `.../v1` → не должно удваиваться.
        assert_eq!(api_base("http://h:8080/v1"), "http://h:8080");
        assert_eq!(api_base("http://h:8080/v1/"), "http://h:8080");
        assert_eq!(api_base("  http://h:8080/v1  "), "http://h:8080");
        // Путь, не оканчивающийся на /v1, не трогаем (кроме хвостового слеша).
        assert_eq!(api_base("http://h:8080/api"), "http://h:8080/api");
    }
}

/// Прод-эмбеддер для live-тестов (игнорируемых по умолчанию): bge-m3/1024 на актуальном LLM-сервере,
/// хост переопределяется `NEXUS_EMBED_URL`. Один помощник вместо хардкодов по тестам —
/// при переезде сервера live-сьют переключается в одном месте (грабля 2026-06-11: три теста
/// остались на стёртом 192.168.0.29 и молча падали бы по сети).
#[cfg(any(test, feature = "test-util"))]
pub fn live_test_embedder() -> OpenAiEmbedder {
    let url =
        std::env::var("NEXUS_EMBED_URL").unwrap_or_else(|_| "http://192.168.0.31:8083".into());
    OpenAiEmbedder::new(
        &crate::net::GuardedClient::unchecked(),
        crate::net::EgressFeature::Embed,
        &url,
        "bge-m3",
        LIVE_EMBED_DIM,
        default_prefixes("bge-m3"),
    )
}

/// Размерность прод-эмбеддера live-тестов (bge-m3) — для `VectorIndex::open` тех же тестов.
#[cfg(any(test, feature = "test-util"))]
pub const LIVE_EMBED_DIM: usize = 1024;

/// Ошибки AI-слоя.
#[derive(Debug, Error)]
pub enum AiError {
    #[error("http: {0}")]
    Http(String),
    #[error("некорректный ответ модели: {0}")]
    BadResponse(String),
    #[error("размерность вектора: ожидалось {expected}, получено {got}")]
    DimMismatch { expected: usize, got: usize },
    /// Эмбеддер вернул не столько векторов, сколько было входов (находка аудита: рассинхрон молча
    /// обрезал бы `zip` при записи в usearch → чанки без вектора). Контракт: ровно N векторов на N входов.
    #[error("число эмбеддингов: ожидалось {expected}, получено {got}")]
    CountMismatch { expected: usize, got: usize },
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
    /// Tool-capable провайдер цикла агента (AGENT-1, I-5/ADR-005). **`None` на десктопе/сегодня** —
    /// конструируется ТОЛЬКО в `nexus-agentd`. НЕ маршрутизируется через `chat`/`chat_fast`/`chat_util`
    /// (те остаются `ChatProvider`, tool-free): отдельный канал, чтобы tool-calling не протекал в chat/web.
    pub agent_tools: Option<Arc<dyn tools::ToolCapableProvider>>,
    /// Политика эгресса ядра — единый экземпляр приложения (см. `AppState::egress_policy`).
    pub policy: Arc<crate::net::EgressPolicy>,
}
