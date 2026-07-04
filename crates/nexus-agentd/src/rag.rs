//! RAG + память агента headless (vault-состояние, отделено от wiring `main.rs` — R-11).
//!
//! Держит остаток CORE-2a: канонный `reconcile_embedding_model` + открытие четырёх usearch-индексов
//! поверх канонного эмбеддера [`nexus_core::bootstrap::EmbeddingBootstrap`] (R-3a). Сборка самих
//! провайдеров (chat/fast/util/tools/embedding) — КАНОН `nexus_core::bootstrap::ProviderSet`; здесь
//! только vault-производные (chunks + индексы).

use std::path::Path;
use std::sync::Arc;

use nexus_core::ai::EmbeddingProvider;
use nexus_core::db::Database;
use nexus_core::vector::VectorIndex;

/// RAG + ПАМЯТЬ агента headless (AGENT-MEM-1): note-RAG индекс + ТРИ индекса памяти (переписка/факты/
/// эпизоды) поверх КАНОННОГО эмбеддера ([`nexus_core::bootstrap::EmbeddingBootstrap`], R-3a). Здесь
/// остаётся vault-состояние: (1) канонный `reconcile_embedding_model` (CORE-2a #2, R-3d «полная
/// чистка») ДО открытия индексов — stale-производные под другой моделью/dim (chunks + все индексы)
/// сбрасываются, иначе запрос новой моделью против старого индекса → `DimMismatch`/семантический
/// мусор; (2) открытие всех четырёх индексов (десктоп держит их в VaultContext, agentd читает
/// память тем же эмбеддером).
pub(crate) struct RagBundle {
    pub(crate) embedder: Arc<dyn EmbeddingProvider>,
    pub(crate) vectors: Arc<VectorIndex>,
    pub(crate) chat_vectors: Arc<VectorIndex>,
    pub(crate) memory_vectors: Arc<VectorIndex>,
    pub(crate) episode_vectors: Arc<VectorIndex>,
}

pub(crate) async fn build_rag_min(
    db: &Database,
    root: &Path,
    eb: nexus_core::bootstrap::EmbeddingBootstrap,
) -> Option<RagBundle> {
    // CORE-2a #2 (R-3d): сверяем производные с активной моделью/dim ДО открытия. Смена → полная
    // чистка (chunks + все индекс-файлы; перезаполнятся индексатором/бэкфиллом); та же модель —
    // строгий no-op. Ошибка БД → RAG off (не открываем потенциально несовместимые индексы).
    let reindex = nexus_core::vector::reconcile_embedding_model(db, root, &eb.model, eb.dim)
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile embedding-модели не удался — RAG off"))
        .ok()?;

    let nexus = root.join(".nexus");
    let open = |name: &str| {
        VectorIndex::open(nexus.join(name), eb.dim)
            .map_err(
                |e| tracing::warn!(error = %e, index = name, "usearch open не удался — RAG off"),
            )
            .ok()
            .map(Arc::new)
    };
    let vectors = open("vectors.usearch")?;
    let chat_vectors = open("chat_vectors.usearch")?;
    let memory_vectors = open("memory_vectors.usearch")?;
    let episode_vectors = open("episode_vectors.usearch")?;

    tracing::info!(model = %eb.model, dim = eb.dim, reindex, "RAG + память агента включены (headless)");
    Some(RagBundle {
        embedder: eb.embedder,
        vectors,
        chat_vectors,
        memory_vectors,
        episode_vectors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::ai::LocalConfig;
    use nexus_core::net::{EgressAudit, EgressPolicy};
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    // ── R-3a: ХАРАКТЕРИЗАЦИЯ сборки провайдеров (REFACTOR-PLAN §3, thermo-смелл №3) ────────────────
    //
    // Фикстура «до»: снимки ВСЕХ конфиг-наблюдаемых параметров провайдеров (`debug_params`) были
    // СНЯТЫ со старых строителей agentd (`build_chat_min`/`build_util_chat_min`/`build_agent_tools_min`
    // /embedder-часть `build_rag_min` + композиция `run()`) в КОММИТЕ 1 этого среза (двухкоммитный
    // приём R-2) — и НЕ менялись при переключении сборки на канон `nexus_core::bootstrap::ProviderSet`
    // (коммит 2, этот код): байт-идентичность канона доказана, не задекларирована.
    // Строки-снимки НЕ «пере-снимать» при рефакторе канона — они и есть контракт.

    /// «Полный» конфиг: chat+fast+embedding, модели заданы, dim задан (без сетевой пробы),
    /// таймауты/температуры дефолтные.
    const BOOT_CFG_FULL: &str = r#"{
      "ai": {
        "chat":      { "url": "http://192.168.0.28:8080", "model": "qwen3-30b", "context_window": 32768 },
        "fast":      { "url": "http://192.168.0.28:8084", "model": "gemma-4b" },
        "embedding": { "url": "http://192.168.0.28:8083", "model": "bge-m3", "dim": 1024 }
      }
    }"#;

    /// Без `ai.fast`: chat_util обязан упасть в fallback на chat_fast (композиция `run()`);
    /// embedding — nomic (характеризует task-префиксы).
    const BOOT_CFG_NO_FAST: &str = r#"{
      "ai": {
        "chat":      { "url": "http://127.0.0.1:9101", "model": "qwen3-30b" },
        "embedding": { "url": "http://127.0.0.1:9103", "model": "nomic-embed-text", "dim": 768 }
      }
    }"#;

    /// Без `ai.embedding`: RAG off, chat+fast живут.
    const BOOT_CFG_NO_EMBEDDING: &str = r#"{
      "ai": {
        "chat": { "url": "http://127.0.0.1:9101", "model": "qwen3-30b" },
        "fast": { "url": "http://127.0.0.1:9104", "model": "gemma-4b" }
      }
    }"#;

    /// Кастомные таймауты/температуры/ретраи ВЕЗДЕ; модели НЕ заданы (дефолты "chat"/"fast"/
    /// "embedding"); chat-url с хвостом `/v1` (характеризует нормализацию `api_base`).
    const BOOT_CFG_CUSTOM: &str = r#"{
      "ai": {
        "chat": {
          "url": "http://127.0.0.1:9201/v1",
          "connect_timeout_secs": 5,
          "first_token_timeout_secs": 45,
          "idle_timeout_secs": 10,
          "retry_attempts": 7,
          "temperature": 0.9
        },
        "fast": {
          "url": "http://127.0.0.1:9202",
          "connect_timeout_secs": 2,
          "first_token_timeout_secs": 20,
          "idle_timeout_secs": 4,
          "retry_attempts": 1,
          "temperature": 0.05
        },
        "embedding": { "url": "http://127.0.0.1:9203", "dim": 512, "timeout_secs": 120 }
      }
    }"#;

    /// Пустой конфиг: ни одного провайдера.
    const BOOT_CFG_EMPTY: &str = r#"{}"#;

    fn boot_cfg(json: &str) -> LocalConfig {
        LocalConfig::parse(json).expect("фикстурный конфиг валиден")
    }

    fn boot_edges() -> (Arc<EgressPolicy>, Arc<EgressAudit>) {
        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        (policy, Arc::new(EgressAudit::default()))
    }

    /// Сборка ТЕКУЩИМ путём agentd — теперь это КАНОН `bootstrap::ProviderSet::from_config` с
    /// опциями FULL, как в `run()` (в коммите 1 здесь была реплика старой композиции
    /// `build_chat_min` → `build_util_chat_min` + fallback → `build_agent_tools_min`;
    /// ассерты тестов НЕ менялись при переключении).
    async fn build_current_way(cfg_json: &str) -> nexus_core::bootstrap::ProviderSet {
        let cfg = boot_cfg(cfg_json);
        let (policy, audit) = boot_edges();
        nexus_core::bootstrap::ProviderSet::from_config(
            &cfg,
            &policy,
            &audit,
            nexus_core::bootstrap::ProviderSetOptions::FULL,
        )
        .await
    }

    /// Эмбеддер ТЕКУЩИМ путём agentd: канонный `EmbeddingBootstrap` СКВОЗЬ `build_rag_min`
    /// (reconcile+usearch на временном vault — живой RAG-путь `run()`; dim задан в фикстурах →
    /// сетевой пробы нет). В коммите 1 тот же путь шёл через старый монолитный `build_rag_min`.
    async fn build_embedder_current_way(cfg_json: &str) -> Option<Arc<dyn EmbeddingProvider>> {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let db = Database::open(root.join(".nexus").join("nexus.db"))
            .await
            .unwrap();
        let eb = build_current_way(cfg_json).await.embedding?;
        build_rag_min(&db, &root, eb).await.map(|r| r.embedder)
    }

    /// Полный конфиг: пара chat-провайдеров — один сервер/модель/температура/таймауты/ретрай,
    /// различие ТОЛЬКО в reasoning (normal ON, fast OFF).
    #[tokio::test]
    async fn boot_chat_pair_full_config() {
        let p = build_current_way(BOOT_CFG_FULL).await;
        assert_eq!(
            p.chat.expect("ai.chat → провайдер").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8080/v1/chat/completions", model: "qwen3-30b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: true }"#
        );
        assert_eq!(
            p.chat_fast.expect("ai.chat → быстрый").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8080/v1/chat/completions", model: "qwen3-30b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: false }"#
        );
    }

    /// Полный конфиг: утилитарная модель из `ai.fast` — свой сервер/модель, ВСЕГДА без reasoning.
    #[tokio::test]
    async fn boot_util_chat_full_config() {
        let p = build_current_way(BOOT_CFG_FULL).await;
        assert_eq!(
            p.chat_util.expect("ai.fast → утилитарная").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8084/v1/chat/completions", model: "gemma-4b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: false }"#
        );
    }

    /// Полный конфиг: tool-capable провайдер агента — тот же ai.chat-хост/модель, БЕЗ retry-поля
    /// (повторами заведует цикл агента), таймауты стрима из конфига.
    #[tokio::test]
    async fn boot_agent_tools_full_config() {
        let p = build_current_way(BOOT_CFG_FULL).await;
        assert_eq!(
            p.agent_tools
                .expect("ai.chat → tool-провайдер")
                .debug_params(),
            r#"OpenAiToolProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8080/v1/chat/completions", model: "qwen3-30b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s }"#
        );
    }

    /// Кастомные таймауты: ВСЕ INFER-CFG параметры конфига доезжают до провайдеров (connect/
    /// first_token/idle/retry/temperature), дефолт-модели "chat"/"fast", `/v1`-хвост не удваивается.
    #[tokio::test]
    async fn boot_custom_timeouts_reach_providers() {
        let p = build_current_way(BOOT_CFG_CUSTOM).await;
        assert_eq!(
            p.chat.expect("ai.chat → провайдер").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=5s)", feature: Chat, endpoint: "http://127.0.0.1:9201/v1/chat/completions", model: "chat", temperature: 0.9, first_token_timeout: 45s, idle_timeout: 10s, retry: RetryPolicy { max_attempts: 7, base: 300ms, cap: 2s }, enable_thinking: true }"#
        );
        assert_eq!(
            p.chat_util.expect("ai.fast → утилитарная").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=2s)", feature: Chat, endpoint: "http://127.0.0.1:9202/v1/chat/completions", model: "fast", temperature: 0.05, first_token_timeout: 20s, idle_timeout: 4s, retry: RetryPolicy { max_attempts: 1, base: 300ms, cap: 2s }, enable_thinking: false }"#
        );
        assert_eq!(
            p.agent_tools
                .expect("ai.chat → tool-провайдер")
                .debug_params(),
            r#"OpenAiToolProvider { client: "for_chat(connect_timeout=5s)", feature: Chat, endpoint: "http://127.0.0.1:9201/v1/chat/completions", model: "chat", temperature: 0.9, first_token_timeout: 45s, idle_timeout: 10s }"#
        );
    }

    /// Без `ai.fast`: утилитарный канал = ТОТ ЖЕ Arc, что chat_fast (fallback композиции — дайджест/
    /// примитивы не дохнут без отдельной мелкой модели).
    #[tokio::test]
    async fn boot_util_falls_back_to_chat_fast() {
        let p = build_current_way(BOOT_CFG_NO_FAST).await;
        let fast = p.chat_fast.expect("ai.chat → быстрый");
        let util = p.chat_util.expect("fallback → chat_fast");
        assert!(
            Arc::ptr_eq(&fast, &util),
            "без ai.fast chat_util обязан быть ТЕМ ЖЕ провайдером, что chat_fast"
        );
    }

    /// Пустой конфиг: ни одного провайдера (vault работает без AI — local-first).
    #[tokio::test]
    async fn boot_empty_config_builds_nothing() {
        let p = build_current_way(BOOT_CFG_EMPTY).await;
        assert!(p.chat.is_none(), "нет ai.chat → нет chat");
        assert!(p.chat_fast.is_none(), "нет ai.chat → нет chat_fast");
        assert!(p.chat_util.is_none(), "нет ai.fast И нет chat_fast → None");
        assert!(p.agent_tools.is_none(), "нет ai.chat → нет tool-провайдера");
    }

    /// Полный конфиг: эмбеддер — url/model/dim/таймаут guarded-клиента; bge → БЕЗ task-префиксов.
    #[tokio::test]
    async fn boot_embedder_full_config() {
        let e = build_embedder_current_way(BOOT_CFG_FULL)
            .await
            .expect("ai.embedding+dim → эмбеддер без пробы");
        assert_eq!(
            e.debug_params(),
            r#"OpenAiEmbedder { client: "for_embedding(timeout=60s)", feature: Embed, endpoint: "http://192.168.0.28:8083/v1/embeddings", model: "bge-m3", dim: 1024, query_prefix: "", document_prefix: "" }"#
        );
    }

    /// nomic-модель: task-префиксы `search_query:`/`search_document:` применены (default_prefixes).
    #[tokio::test]
    async fn boot_embedder_nomic_prefixes() {
        let e = build_embedder_current_way(BOOT_CFG_NO_FAST)
            .await
            .expect("ai.embedding+dim → эмбеддер");
        assert_eq!(
            e.debug_params(),
            r#"OpenAiEmbedder { client: "for_embedding(timeout=60s)", feature: Embed, endpoint: "http://127.0.0.1:9103/v1/embeddings", model: "nomic-embed-text", dim: 768, query_prefix: "search_query: ", document_prefix: "search_document: " }"#
        );
    }

    /// Кастомный embedding-таймаут + дефолт-модель "embedding" доезжают до эмбеддера.
    #[tokio::test]
    async fn boot_embedder_custom_timeout() {
        let e = build_embedder_current_way(BOOT_CFG_CUSTOM)
            .await
            .expect("ai.embedding+dim → эмбеддер");
        assert_eq!(
            e.debug_params(),
            r#"OpenAiEmbedder { client: "for_embedding(timeout=120s)", feature: Embed, endpoint: "http://127.0.0.1:9203/v1/embeddings", model: "embedding", dim: 512, query_prefix: "", document_prefix: "" }"#
        );
    }

    /// Без `ai.embedding` эмбеддера нет (RAG off) — chat-провайдеры при этом живут (см. фикстуру).
    #[tokio::test]
    async fn boot_no_embedding_no_embedder() {
        assert!(
            build_embedder_current_way(BOOT_CFG_NO_EMBEDDING)
                .await
                .is_none(),
            "нет ai.embedding → нет эмбеддера"
        );
    }
}
