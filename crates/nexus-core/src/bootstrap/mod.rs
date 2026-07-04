//! bootstrap — КАНОН сборки LLM-провайдеров из `.nexus/local.json` (R-3a, REFACTOR-PLAN §3).
//!
//! До R-3a сборка провайдеров была продублирована ×3 (thermo-смелл №3): desktop
//! `commands/vault.rs::build_chat/build_util_chat/build_rag` (+hot-apply зеркало в `settings.rs`),
//! agentd `main.rs::build_*_min`, cli `agent.rs` (частично через `ai::tools::build_agent_tool_provider`).
//! Копии уже дрейфовали косметически (тексты логов); политика «PREFER copy over expose» отменена
//! владельцем (§8.8). Канон живёт в core (сервер-паритет, решение §8.3): БЕЗ tauri-типов, все
//! app-специфики — параметрами ([`ProviderSetOptions`], прецедент `IndexerHooks`).
//!
//! **Байт-идентичность старым строителям доказана характеризацией** (nexus-agentd
//! `tests::boot_*` — R-3a; nexus-desktop `commands::vault::tests::boot_*` — R-3b; nexus-cli
//! `agent::tests::boot_*` — R-3c): снимки всех конфиг-наблюдаемых параметров сняты со СТАРОГО
//! кода (коммит 1) и не менялись при переключении на канон (коммит 2). Не «пере-снимать» их при
//! рефакторе — они и есть контракт.
//!
//! Внедрение — строитель-за-строителем × бинарь-за-бинарём: R-3a переключил agentd, R-3b —
//! desktop `open_vault`, R-3c — cli `build_deps` (`nexus agent`/`nexus acp`; профиль
//! `{agent_tools: true, embedding: false}` — chat-каналы канона cli не использует).
//!
//! ЗА КАДРОМ канона (намеренно, границы среза):
//! - `reconcile_embedding_model` + открытие usearch-индексов — у вызывающего (reconcile — отдельный
//!   декларируемый срез: у desktop/agentd он совпадает, но это семантика vault-состояния, не сборки);
//! - hot-apply провайдеров desktop (`set_ai_config`) — НАМЕРЕННО особый путь (EndpointDto из UI не
//!   несёт таймаутов → единый `saved_chat`-профиль + URL-fallback fast→chat). Решение R-3b: НЕ
//!   переключён — байт-в-байт с каноном невозможен (канон дал бы fast'у собственный профиль из
//!   `ai.fast` и fallback ТЕМ ЖЕ Arc); дельта запинена фикстурной таблицей
//!   nexus-desktop `commands::settings::tests::hot_*`, унификация — отдельным декларируемым срезом.

use std::sync::Arc;
use std::time::Duration;

use crate::ai::tools::ToolCapableProvider;
use crate::ai::{
    self, ChatConfig, ChatProvider, EmbeddingProvider, LocalConfig, OpenAiChatProvider,
    OpenAiEmbedder,
};
use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient};

/// Таймаут пробы размерности эмбеддинга (`probe_dim`, когда `embedding.dim` не задан) — 30 с,
/// как было захардкожено в обоих вызывателях (desktop `build_rag` / agentd `build_rag_min`).
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Собранный embedding-фундамент: провайдер + разрешённые model/dim. `model`/`dim` отдаются
/// отдельно, потому что вызывающему они нужны ДАЛЬШЕ сборки провайдера: `reconcile_embedding_model`
/// (сброс stale-индексов при смене модели, §6.5) и `VectorIndex::open(…, dim)` — оба вне канона.
pub struct EmbeddingBootstrap {
    pub embedder: Arc<dyn EmbeddingProvider>,
    /// Имя модели ПОСЛЕ дефолта (`"embedding"`, если в конфиге не задано).
    pub model: String,
    /// Размерность: из конфига либо пробным эмбеддингом у сервера (§6.5 — не хардкод).
    pub dim: usize,
}

/// Какие провайдеры строить — НАМЕРЕННЫЕ различия вызывателей выражены параметрами, а не копиями
/// строителей (R-3a: «намеренное сохраняется параметрами»).
#[derive(Debug, Clone, Copy)]
pub struct ProviderSetOptions {
    /// Строить ли tool-capable провайдер агента (AGENT-1, I-5). agentd/cli — да (агенту нечем думать
    /// без него); **desktop держит `None` намеренно** (tool-calling не протекает в chat-путь app;
    /// per-run провайдер агента desktop строит сам через `ai::tools::build_agent_tool_provider`).
    pub agent_tools: bool,
    /// Строить ли embedding (при отсутствии `embedding.dim` уходит СЕТЕВАЯ проба размерности).
    /// desktop/agentd — да (RAG/память); cli — нет (агент без RAG-фундамента).
    pub embedding: bool,
}

impl ProviderSetOptions {
    /// Полный набор (agentd; desktop с R-3b строит с `agent_tools: false` — I-5).
    pub const FULL: Self = Self {
        agent_tools: true,
        embedding: true,
    };
}

/// Причина, почему `.nexus/local.json` НЕ дал конфига, — структурная основа канона №2 для
/// проекций вызывателей с РАЗНОЙ эргономикой ошибок: Option-канон [`load_local_config`]
/// (desktop/agentd: нет конфига → AI off, vault живёт — local-first) и онбординг-Result cli
/// (`nexus agent`: конфиг ОБЯЗАТЕЛЕН, тексты различают «нет файла» и «битый JSON» — прежние,
/// запинены характеризацией cli `boot_*`).
#[derive(Debug)]
pub enum LocalConfigError {
    /// Файл не прочитался (обычно его просто нет — онбординг ещё не создал `.nexus/local.json`).
    Unreadable,
    /// Файл есть, но JSON не разобрался (ошибка парсера — в Display через `AiError::Config`).
    Parse(ai::AiError),
}

/// КАНОН №2, структурная форма (R-3c): чтение+разбор `.nexus/local.json` — ОДИН раз на открытие
/// (кросс-план #8), с различением причин отказа для проекций (см. [`LocalConfigError`]).
pub async fn read_local_config(root: &std::path::Path) -> Result<LocalConfig, LocalConfigError> {
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .map_err(|_| LocalConfigError::Unreadable)?;
    LocalConfig::parse(&raw).map_err(LocalConfigError::Parse)
}

/// КАНОН №2 (R-3b), Option-проекция: `None` — файла нет / битый JSON (AI отключается, vault живёт
/// без AI — local-first). Бывшие desktop-реплики `commands/vault.rs::load_local_config` и
/// `commands/agent.rs::load_local_config` переключены в R-3b (у второй дрейфовал текст warn-лога —
/// «agent_run: local.json не распарсен — дефолты»; канон говорит единым текстом ниже); реплика
/// agentd `main.rs` (тело было байт-идентично канону) переключена в R-3c; cli — онбординг-проекция
/// над [`read_local_config`] (Result с прежними текстами, warn не пишет — ошибка уходит владельцу).
pub async fn load_local_config(root: &std::path::Path) -> Option<LocalConfig> {
    match read_local_config(root).await {
        Ok(cfg) => Some(cfg),
        Err(LocalConfigError::Unreadable) => None,
        Err(LocalConfigError::Parse(e)) => {
            tracing::warn!(error = %e, "local.json: разбор не удался — AI отключён");
            None
        }
    }
}

/// Канонический набор LLM-провайдеров vault'а — то, что композиционный корень кладёт в
/// [`crate::ai::AIClient`] (+ `embedding` для RAG-фундамента вызывающего).
#[derive(Default)]
pub struct ProviderSet {
    /// Chat с reasoning (RAG-чат). `None` — нет `ai.chat` / клиент не построился.
    pub chat: Option<Arc<dyn ChatProvider>>,
    /// «Быстрый» chat без reasoning на ОСНОВНОЙ модели (R2: дайджест/примитивы). Строится вместе с `chat`.
    pub chat_fast: Option<Arc<dyn ChatProvider>>,
    /// Утилитарная мелкая модель из `ai.fast` (без reasoning). Нет секции / не построился →
    /// fallback на `chat_fast` (ТОТ ЖЕ Arc) — композиция, единая у desktop/agentd до канона.
    pub chat_util: Option<Arc<dyn ChatProvider>>,
    /// Tool-capable провайдер агента (AGENT-1). Строится ТОЛЬКО при `opts.agent_tools`.
    pub agent_tools: Option<Arc<dyn ToolCapableProvider>>,
    /// Embedding-фундамент (провайдер + model/dim для reconcile/индексов вызывающего). Строится
    /// ТОЛЬКО при `opts.embedding`.
    pub embedding: Option<EmbeddingBootstrap>,
}

impl ProviderSet {
    /// КАНОН №1 (R-3a): сборка всех LLM-провайдеров из распарсенного конфига. Поведение — байт-в-байт
    /// прежние строители (характеризация agentd `tests::boot_*`): те же фабрики guarded-клиента с теми
    /// же таймаутами конфига, те же дефолты моделей (`"chat"`/`"fast"`/`"embedding"`), тот же выбор
    /// reasoning. Доступность серверов НЕ проверяется (выяснится при первом стриме) — кроме сетевой
    /// пробы размерности embedding при незаданном `dim`.
    ///
    /// Порядок сборки — как в исходных композициях (`open_vault`/agentd `run()`): embedding (возможная
    /// сетевая проба) → chat-пара → утилитарная (+fallback) → tool-провайдер.
    pub async fn from_config(
        cfg: &LocalConfig,
        policy: &Arc<EgressPolicy>,
        audit: &Arc<EgressAudit>,
        opts: ProviderSetOptions,
    ) -> Self {
        let embedding = if opts.embedding {
            build_embedding(cfg, policy, audit).await
        } else {
            None
        };
        let (chat, chat_fast) = match build_chat_pair(cfg, policy, audit) {
            Some((normal, fast)) => (Some(normal), Some(fast)),
            None => (None, None),
        };
        let chat_util = build_util_chat(cfg, policy, audit).or_else(|| chat_fast.clone());
        let agent_tools = if opts.agent_tools {
            ai::tools::build_agent_tool_provider(cfg, policy, audit)
        } else {
            None
        };
        Self {
            chat,
            chat_fast,
            chat_util,
            agent_tools,
            embedding,
        }
    }
}

/// INFER-CFG: применяет к chat-провайдеру таймауты стрима/retry из `ChatConfig`
/// (first_token/idle/retry). Температуру задаёт уже `new(..., Some(c.temperature()))`;
/// connect-таймаут — у guarded-клиента. Бывшие реплики: desktop `vault.rs::apply_chat_cfg`
/// и agentd `apply_chat_cfg`.
fn apply_chat_cfg(p: OpenAiChatProvider, c: &ChatConfig) -> OpenAiChatProvider {
    p.with_first_token_timeout(c.first_token_timeout())
        .with_idle_timeout(c.idle_timeout())
        .with_retry_attempts(c.retry_attempts())
}

/// Пара chat-провайдеров из `ai.chat`: `(обычный с reasoning, быстрый без reasoning)`. Оба — тот же
/// сервер/модель; быстрый шлёт `enable_thinking=false` (R2). `None` — нет секции / guarded-клиент
/// не построился. Бывшие реплики: desktop `build_chat`, agentd `build_chat_min`.
fn build_chat_pair(
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<(Arc<dyn ChatProvider>, Arc<dyn ChatProvider>)> {
    let chat = cfg.ai.chat.as_ref()?;
    let model = chat.model.clone().unwrap_or_else(|| "chat".to_string());
    // INFER-CFG: connect-таймаут и температура/таймауты стрима/retry из конфига (дефолты при отсутствии).
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone(), chat.connect_timeout())
        .map_err(|e| tracing::warn!(error = %e, "chat-провайдер не инициализирован"))
        .ok()?;
    let normal = apply_chat_cfg(
        OpenAiChatProvider::new(
            &guarded,
            EgressFeature::Chat,
            &chat.url,
            &model,
            Some(chat.temperature()),
        ),
        chat,
    );
    let fast = apply_chat_cfg(
        OpenAiChatProvider::new(
            &guarded,
            EgressFeature::Chat,
            &chat.url,
            &model,
            Some(chat.temperature()),
        ),
        chat,
    )
    .without_reasoning();
    tracing::info!(model = %model, "chat-провайдеры включены (reasoning + fast)");
    Some((Arc::new(normal), Arc::new(fast)))
}

/// Утилитарная мелкая модель из `ai.fast` (для примитивов: inline/судья/сводка). ВСЕГДА
/// `without_reasoning()`: примитивам CoT не нужен, а на `ai.fast` может жить reasoning-модель
/// (баг 2026-06-11: gemma12 думала ~40 с над 6-словной сводкой R1). `None` — секции нет / клиент
/// не построился → вызывающий (или [`ProviderSet::from_config`]) делает fallback на `chat_fast`.
/// Бывшие реплики: desktop `build_util_chat`, agentd `build_util_chat_min` (тексты логов копий
/// дрейфовали — «gemma-fast» vs «chat_fast»; канон говорит «chat_fast», фактическое имя fallback-поля).
fn build_util_chat(
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<Arc<dyn ChatProvider>> {
    let fast = cfg.ai.fast.as_ref()?;
    let model = fast.model.clone().unwrap_or_else(|| "fast".to_string());
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone(), fast.connect_timeout())
        .map_err(
            |e| tracing::warn!(error = %e, "ai.fast: провайдер не создан — fallback на chat_fast"),
        )
        .ok()?;
    let provider = apply_chat_cfg(
        OpenAiChatProvider::new(
            &guarded,
            EgressFeature::Chat,
            &fast.url,
            &model,
            Some(fast.temperature()),
        ),
        fast,
    )
    .without_reasoning();
    tracing::info!(model = %model, url = %fast.url, "ai.fast (утилитарная модель) включена");
    Some(Arc::new(provider))
}

/// Embedding-фундамент из `ai.embedding`: размерность из конфига либо сетевой пробой (§6.5), guarded-
/// клиент с таймаутом конфига, task-префиксы по модели (`ai::default_prefixes`). `None` — нет секции /
/// проба или клиент не удались (вызывающий выключает RAG; тексты логов копий дрейфовали — «RAG
/// отключён» vs «RAG off»; канон говорит «embedding выключен», решение об off RAG — у вызывающего).
/// Бывшие реплики: embedder-часть desktop `build_rag` и agentd `build_rag_min` (reconcile + открытие
/// индексов остались у вызывателей — вне среза R-3a).
async fn build_embedding(
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<EmbeddingBootstrap> {
    let emb = cfg.ai.embedding.as_ref()?;
    let model = emb.model.clone().unwrap_or_else(|| "embedding".to_string());

    let dim = match emb.dim {
        Some(d) => d,
        None => {
            let probe = GuardedClient::for_probe(policy.clone(), audit.clone(), PROBE_TIMEOUT)
                .map_err(
                    |e| tracing::warn!(error = %e, "probe-клиент не построился — embedding выключен"),
                )
                .ok()?;
            OpenAiEmbedder::probe_dim(&probe, &emb.url, &model)
                .await
                .map_err(
                    |e| tracing::warn!(error = %e, "проба размерности не удалась — embedding выключен"),
                )
                .ok()?
        }
    };

    let guarded = GuardedClient::for_embedding(policy.clone(), audit.clone(), emb.timeout())
        .map_err(|e| tracing::warn!(error = %e, "эмбеддер не инициализирован — embedding выключен"))
        .ok()?;
    let embedder = OpenAiEmbedder::new(
        &guarded,
        EgressFeature::Embed,
        &emb.url,
        &model,
        dim,
        ai::default_prefixes(&model),
    );
    Some(EmbeddingBootstrap {
        embedder: Arc::new(embedder),
        model,
        dim,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn edges() -> (Arc<EgressPolicy>, Arc<EgressAudit>) {
        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        (policy, Arc::new(EgressAudit::default()))
    }

    fn full_cfg() -> LocalConfig {
        LocalConfig::parse(
            r#"{
              "ai": {
                "chat":      { "url": "http://127.0.0.1:9101", "model": "q" },
                "fast":      { "url": "http://127.0.0.1:9102", "model": "f" },
                "embedding": { "url": "http://127.0.0.1:9103", "model": "e", "dim": 8 }
              }
            }"#,
        )
        .unwrap()
    }

    /// Полный конфиг + FULL-опции → все пять каналов собраны. (Байт-точные параметры пинают
    /// характеризационные тесты agentd `boot_*` — здесь состав/гейтинг.)
    #[tokio::test]
    async fn full_options_build_everything() {
        let (policy, audit) = edges();
        let s =
            ProviderSet::from_config(&full_cfg(), &policy, &audit, ProviderSetOptions::FULL).await;
        assert!(s.chat.is_some() && s.chat_fast.is_some() && s.chat_util.is_some());
        assert!(s.agent_tools.is_some());
        let eb = s.embedding.expect("dim задан → без пробы");
        assert_eq!((eb.model.as_str(), eb.dim), ("e", 8));
        assert_eq!(eb.embedder.dim(), 8);
    }

    /// НАМЕРЕННЫЕ различия вызывателей — параметрами: desktop-профиль (`agent_tools: false`) не строит
    /// tool-провайдер; cli-профиль (`embedding: false`) не строит embedding (и не ходит в пробу).
    #[tokio::test]
    async fn options_gate_agent_tools_and_embedding() {
        let (policy, audit) = edges();
        let desktop = ProviderSet::from_config(
            &full_cfg(),
            &policy,
            &audit,
            ProviderSetOptions {
                agent_tools: false,
                embedding: true,
            },
        )
        .await;
        assert!(desktop.agent_tools.is_none(), "desktop держит None (I-5)");
        assert!(desktop.embedding.is_some());

        let cli = ProviderSet::from_config(
            &full_cfg(),
            &policy,
            &audit,
            ProviderSetOptions {
                agent_tools: true,
                embedding: false,
            },
        )
        .await;
        assert!(
            cli.embedding.is_none(),
            "embedding не строится (и не пробится) без опции"
        );
        assert!(cli.agent_tools.is_some());
    }

    /// Без `ai.fast` утилитарный канал = ТОТ ЖЕ Arc, что `chat_fast` (fallback-композиция канона —
    /// как было в `open_vault`/agentd `run()`).
    #[tokio::test]
    async fn util_falls_back_to_chat_fast() {
        let (policy, audit) = edges();
        let cfg = LocalConfig::parse(
            r#"{ "ai": { "chat": { "url": "http://127.0.0.1:9101", "model": "q" } } }"#,
        )
        .unwrap();
        let s = ProviderSet::from_config(&cfg, &policy, &audit, ProviderSetOptions::FULL).await;
        let fast = s.chat_fast.expect("chat → пара");
        let util = s.chat_util.expect("fallback");
        assert!(Arc::ptr_eq(&fast, &util));
    }

    /// Структурная форма канона №2 (R-3c): `read_local_config` различает «нет файла» и «битый
    /// JSON» — онбординг-проекция cli строит по ним РАЗНЫЕ тексты (сами тексты запинены
    /// характеризацией nexus-cli `agent::tests::boot_*`).
    #[tokio::test]
    async fn read_local_config_distinguishes_unreadable_and_parse() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        assert!(matches!(
            read_local_config(root).await,
            Err(LocalConfigError::Unreadable)
        ));
        std::fs::create_dir_all(root.join(".nexus")).unwrap();
        std::fs::write(root.join(".nexus").join("local.json"), "{ битый").unwrap();
        assert!(matches!(
            read_local_config(root).await,
            Err(LocalConfigError::Parse(_))
        ));
        std::fs::write(root.join(".nexus").join("local.json"), "{}").unwrap();
        assert!(read_local_config(root).await.is_ok());
    }

    /// КАНОН №2 (R-3b): `load_local_config` — нет файла → None, битый JSON → None (warn, AI off),
    /// валидный → Some (распарсенные секции живы).
    #[tokio::test]
    async fn load_local_config_reads_parses_and_degrades() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        assert!(load_local_config(root).await.is_none(), "нет файла → None");
        std::fs::create_dir_all(root.join(".nexus")).unwrap();
        std::fs::write(root.join(".nexus").join("local.json"), "{ битый").unwrap();
        assert!(load_local_config(root).await.is_none(), "битый JSON → None");
        std::fs::write(
            root.join(".nexus").join("local.json"),
            r#"{ "ai": { "chat": { "url": "http://127.0.0.1:9101" } } }"#,
        )
        .unwrap();
        let cfg = load_local_config(root).await.expect("валидный → Some");
        assert!(cfg.ai.chat.is_some());
    }

    /// Пустой конфиг → пустой набор (local-first: vault живёт без AI); Default — то же самое
    /// (путь «нет/битый local.json» у вызывателей).
    #[tokio::test]
    async fn empty_config_builds_nothing() {
        let (policy, audit) = edges();
        let cfg = LocalConfig::parse("{}").unwrap();
        let s = ProviderSet::from_config(&cfg, &policy, &audit, ProviderSetOptions::FULL).await;
        assert!(s.chat.is_none() && s.chat_fast.is_none() && s.chat_util.is_none());
        assert!(s.agent_tools.is_none() && s.embedding.is_none());

        let d = ProviderSet::default();
        assert!(d.chat.is_none() && d.chat_fast.is_none() && d.chat_util.is_none());
        assert!(d.agent_tools.is_none() && d.embedding.is_none());
    }
}
