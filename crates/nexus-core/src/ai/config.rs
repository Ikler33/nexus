//! Локальный конфиг vault (`.nexus/local.json`, в .gitignore — ADR-002): эндпоинты/модели
//! chat и embedding. Ключи здесь НЕ в git; `*.url` валидируются анти-SSRF позже (§11).

use std::time::Duration;

use serde::Deserialize;

use super::{AiError, AiResult};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocalConfig {
    #[serde(default)]
    pub ai: AiConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AiConfig {
    /// Chat-провайдер (Gemma и т.п.) — отдельный хост (ADR-005).
    pub chat: Option<ChatConfig>,
    /// Embedding-провайдер (мультиязычный) — отдельный хост (ADR-005).
    pub embedding: Option<EmbeddingConfig>,
    /// «Быстрая» утилитарная модель (мелкая, напр. Qwen3-4B на отдельном порту) для примитивов
    /// (inline/судья): низкая латентность + разгрузка основной модели. Опционально — нет секции →
    /// fallback на основной chat без reasoning. Non-reasoning-модель → шлём обычный запрос.
    pub fast: Option<ChatConfig>,
    /// Путь к `tokenizer.json` для оценки бюджета контекста (P0-c). `None` → встроенный токенайзер
    /// задеплоенной модели (Qwen3.6-27B). Смена модели = положить новый файл + прописать этот путь,
    /// без пересборки (см. `ai::QwenTokenizer`). Относительный путь резолвится вызывающим.
    #[serde(default)]
    pub tokenizer_path: Option<String>,

    /// **GO-LIVE АКТУАТОРА (AGENT-3e), SAFE BY DEFAULT.** Когда `false` (ДЕФОЛТ) — прогон агента
    /// работает ТОЛЬКО со стаб-инструментами (echo/noop); реальный vault НИКОГДА не затрагивается из
    /// коробки. Когда `true` — [`crate::agent::AgentRunHandler`] регистрирует файловые инструменты-
    /// актуаторы (note.create/edit/set_frontmatter), маршрутизируемые ИСКЛЮЧИТЕЛЬНО через гейт
    /// автономии (`actuator::dispatch_action`). Даже включённый, headless-agentd под `PolicyDefault`
    /// авто-применяет лишь Auto-тир на `autonomy=auto`-прогоне; Confirm-тир всегда предлагается и
    /// auto-DENY-отклоняется (нет UI/контрол-плейна). Владелец opt-in'ит осознанно.
    #[serde(default)]
    pub agent_actuator_enabled: bool,

    /// Порог «крупной перезаписи» (байт) для гейта актуатора → Confirm-тир (`DispatchPolicy
    /// .overwrite_threshold`). `None` → дефолт [`crate::actuator::OVERWRITE_THRESHOLD`] (64 KiB).
    /// Только при `agent_actuator_enabled=true` имеет эффект.
    #[serde(default)]
    pub agent_overwrite_threshold: Option<usize>,

    /// Кэп кумулятивных авто-применений Auto-тира В ПРОГОНЕ (анти-усталость): за ним даже Auto-тир
    /// форсирует предложение. `None` → дефолт [`AiConfig::DEFAULT_BLAST_RADIUS_CAP`]. Только при
    /// `agent_actuator_enabled=true` имеет эффект.
    #[serde(default)]
    pub agent_blast_radius_cap: Option<u32>,

    /// **SKILL-2: каталог скиллов (SKILL.md) для прогона агента.** Путь к каталогу со скиллами
    /// открытого стандарта SKILL.md (`<dir>/<skill>/SKILL.md`). `None` (ДЕФОЛТ) → агент работает БЕЗ
    /// скиллов (нет меню в контексте, нет `activate_skill`/`read_skill_resource` — поведение без
    /// регрессии). Когда задан → [`crate::agent::AgentRunHandler`] инжектит фенсенное МЕНЮ скиллов
    /// (tier 1) и регистрирует READ-ONLY инструменты раскрытия (tier 2/3). Скиллы лишь читаются;
    /// активация даёт ТОЛЬКО текст-инструкции (capability-гейт — SKILL-3). Относительный путь
    /// резолвится вызывающим относительно vault (рекомендация: `<vault>/.nexus/skills`).
    #[serde(default)]
    pub agent_skills_dir: Option<String>,

    /// **EGR-AGENT: веб-инструменты агента (`web.search`/`web.fetch`).** `None`/`enabled=false` (ДЕФОЛТ) →
    /// агент без веб-доступа. Задан+enabled → composition root включает `EgressFeature::Web` + allowlist
    /// хоста SearXNG и регистрирует read-only веб-инструменты. Эгресс — через `GuardedClient` (web-класс:
    /// SSRF-гард, allowlist, аудит). Только для прогона агента; chat-путь не затрагивает.
    #[serde(default)]
    pub web: Option<WebConfig>,
}

/// Конфиг веб-инструментов агента (EGR-AGENT-2). `url` — база SearXNG (consent-эндпоинт мета-поиска).
#[derive(Debug, Clone, Deserialize)]
pub struct WebConfig {
    /// База SearXNG (например `http://host:8888`). Пусто → web.search не поднимается.
    pub url: String,
    /// Consent-флаг (ДЕФОЛТ false): без него веб-инструменты не регистрируются.
    #[serde(default)]
    pub enabled: bool,
}

impl AiConfig {
    /// Дефолт кэпа blast-radius прогона, если не задан в конфиге (консервативный — небольшая пачка
    /// авто-применений до форс-предложения).
    pub const DEFAULT_BLAST_RADIUS_CAP: u32 = 16;
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatConfig {
    pub url: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub context_window: Option<usize>,

    // --- INFER-CFG: движок-агностичные таймауты/параметры стрима (все Option, serde-default;
    // отсутствие → встроенный дефолт-геттер → zero-config работает как раньше, но с лучшими
    // дефолтами под cold-start V100). Смена llama.cpp → vLLM (Qwen3.6-27B-AWQ на V100) = только
    // эти поля, без кода. См. `docs/dev/chat.md` (профиль свапа).
    /// Таймаут ПЕРВОГО токена (сек): применяется к инициации стрима И ко всем чанкам ДО первого
    /// полученного байта. Переживает cold-start (V100 компилирует ядра 1–3 мин на первом запросе).
    /// `None` → [`ChatConfig::DEFAULT_FIRST_TOKEN_TIMEOUT_SECS`] (300 с).
    #[serde(default)]
    pub first_token_timeout_secs: Option<u64>,
    /// Idle-таймаут стрима ПОСЛЕ первого байта (сек): детект зависшего стрима в steady-state.
    /// `None` → [`ChatConfig::DEFAULT_IDLE_TIMEOUT_SECS`] (90 с).
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    /// Connect-таймаут TCP-коннекта (сек) у guarded-клиента. `None` →
    /// [`ChatConfig::DEFAULT_CONNECT_TIMEOUT_SECS`] (30 с — безопаснее для V100, ок на LAN).
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,
    /// Число попыток ИНИЦИАЦИИ запроса (включая первую). `None` →
    /// [`ChatConfig::DEFAULT_RETRY_ATTEMPTS`] (3).
    #[serde(default)]
    pub retry_attempts: Option<u32>,
    /// Температура сэмплинга. `None` → [`ChatConfig::DEFAULT_TEMPERATURE`] (0.3).
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Сколько токенов резервировать под ОТВЕТ модели (вычитается из окна при сборке контекста).
    /// `None` → [`crate::ai::ContextBudget::DEFAULT_RESERVE_OUTPUT`] (1024).
    #[serde(default)]
    pub reserve_output_tokens: Option<usize>,
}

impl ChatConfig {
    /// Дефолт таймаута первого токена (сек) — переживает cold-start крупных моделей на V100.
    pub const DEFAULT_FIRST_TOKEN_TIMEOUT_SECS: u64 = 300;
    /// Дефолт idle-таймаута стрима после первого байта (сек).
    pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 90;
    /// Дефолт connect-таймаута (сек).
    pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;
    /// Дефолт числа попыток инициации запроса.
    pub const DEFAULT_RETRY_ATTEMPTS: u32 = 3;
    /// Дефолт температуры сэмплинга.
    pub const DEFAULT_TEMPERATURE: f32 = 0.3;

    /// Таймаут первого токена (инициация + чанки ДО первого байта) с дефолтом.
    pub fn first_token_timeout(&self) -> Duration {
        Duration::from_secs(
            self.first_token_timeout_secs
                .unwrap_or(Self::DEFAULT_FIRST_TOKEN_TIMEOUT_SECS),
        )
    }

    /// Idle-таймаут стрима после первого байта с дефолтом.
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(
            self.idle_timeout_secs
                .unwrap_or(Self::DEFAULT_IDLE_TIMEOUT_SECS),
        )
    }

    /// Connect-таймаут с дефолтом (для `GuardedClient::for_chat`).
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(
            self.connect_timeout_secs
                .unwrap_or(Self::DEFAULT_CONNECT_TIMEOUT_SECS),
        )
    }

    /// Число попыток инициации запроса с дефолтом.
    pub fn retry_attempts(&self) -> u32 {
        self.retry_attempts.unwrap_or(Self::DEFAULT_RETRY_ATTEMPTS)
    }

    /// Температура сэмплинга с дефолтом.
    pub fn temperature(&self) -> f32 {
        self.temperature.unwrap_or(Self::DEFAULT_TEMPERATURE)
    }

    /// Резерв токенов под ответ с дефолтом ([`crate::ai::ContextBudget::DEFAULT_RESERVE_OUTPUT`]).
    pub fn reserve_output_tokens(&self) -> usize {
        self.reserve_output_tokens
            .unwrap_or(crate::ai::ContextBudget::DEFAULT_RESERVE_OUTPUT)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    pub url: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Размерность; если не задана — берётся из ответа модели при первом эмбеддинге.
    #[serde(default)]
    pub dim: Option<usize>,
    /// INFER-CFG: общий таймаут эмбеддинг-запроса (сек) у guarded-клиента (батчи бывают тяжёлые;
    /// V100-профиль ставит больше). `None` → [`EmbeddingConfig::DEFAULT_TIMEOUT_SECS`] (60 с).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

impl EmbeddingConfig {
    /// Дефолт таймаута эмбеддинг-запроса (сек).
    pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

    /// Таймаут эмбеддинг-запроса с дефолтом (для `GuardedClient::for_embedding`).
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.unwrap_or(Self::DEFAULT_TIMEOUT_SECS))
    }
}

impl LocalConfig {
    pub fn parse(json: &str) -> AiResult<Self> {
        serde_json::from_str(json).map_err(|e| AiError::Config(e.to_string()))
    }

    /// Хосты явно сконфигурированных `ai.*`-эндпоинтов — для авто-allowlist политики эгресса
    /// (ADR-005-ext E4: «явные `ai.*.url` разрешены», уточнённый AC-SEC-4/E3). Только хост (без
    /// порта/пути) — allowlist exact-host, как у брокера. Невалидные URL пропускаются (провайдер
    /// по ним всё равно не построится; политика — fail-closed).
    pub fn egress_hosts(&self) -> Vec<String> {
        [
            self.ai.chat.as_ref().map(|c| c.url.as_str()),
            self.ai.embedding.as_ref().map(|e| e.url.as_str()),
            self.ai.fast.as_ref().map(|f| f.url.as_str()),
        ]
        .into_iter()
        .flatten()
        .filter_map(|u| {
            reqwest::Url::parse(u)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_json() {
        // Форма из ARCHITECTURE §5 (.nexus/local.json).
        let json = r#"{
          "ai": {
            "chat":      { "url": "http://192.168.0.29:8080", "model": "gemma-4-26B-A4B-it", "context_window": 32768 },
            "embedding": { "url": "http://192.168.0.29:8081", "model": "nomic-embed-text", "dim": 768 },
            "reranker":  { "url": "http://192.168.0.29:8082", "enabled": false }
          },
          "sync": { "remote": null }
        }"#;
        let cfg = LocalConfig::parse(json).unwrap();
        let chat = cfg.ai.chat.unwrap();
        assert_eq!(chat.url, "http://192.168.0.29:8080");
        assert_eq!(chat.context_window, Some(32768));
        let emb = cfg.ai.embedding.unwrap();
        assert_eq!(emb.url, "http://192.168.0.29:8081");
        assert_eq!(emb.dim, Some(768));
    }

    #[test]
    fn tolerates_partial_and_unknown_fields() {
        let cfg = LocalConfig::parse(r#"{"ai":{"embedding":{"url":"http://x:8081"}}}"#).unwrap();
        assert!(cfg.ai.chat.is_none());
        assert_eq!(cfg.ai.embedding.unwrap().dim, None);
    }

    /// E4: авто-allowlist берёт ИМЕННО хосты явных `ai.*.url` (chat/embedding/fast), без порта;
    /// битый URL пропускается, пустой конфиг → пусто (fail-closed).
    #[test]
    fn egress_hosts_extracts_explicit_ai_hosts() {
        let cfg = LocalConfig::parse(
            r#"{"ai":{
                "chat":      { "url": "https://api.example.com/v1" },
                "embedding": { "url": "http://192.168.0.29:8083" },
                "fast":      { "url": "not a url" }
            }}"#,
        )
        .unwrap();
        let hosts = cfg.egress_hosts();
        assert_eq!(
            hosts,
            vec!["api.example.com".to_string(), "192.168.0.29".to_string()]
        );
        assert!(LocalConfig::default().egress_hosts().is_empty());
    }

    /// P0-c: `ai.tokenizer_path` парсится (смена модели токенайзера = файл+конфиг); по умолчанию None.
    #[test]
    fn parses_tokenizer_path() {
        let cfg = LocalConfig::parse(r#"{"ai":{"tokenizer_path":"/vault/.nexus/tokenizer.json"}}"#)
            .unwrap();
        assert_eq!(
            cfg.ai.tokenizer_path.as_deref(),
            Some("/vault/.nexus/tokenizer.json")
        );
        // Нет ключа → None (встроенный токенайзер задеплоенной модели).
        assert!(LocalConfig::parse(r#"{"ai":{}}"#)
            .unwrap()
            .ai
            .tokenizer_path
            .is_none());
    }

    /// AGENT-3e SAFE-BY-DEFAULT: флаг актуатора по умолчанию FALSE (нет ключа → стабы, реальный vault
    /// не затронут). Связанные пороги по умолчанию None (берётся ядровый дефолт). Включается явно.
    #[test]
    fn agent_actuator_disabled_by_default() {
        // Пустой ai-блок → флаг false, пороги None.
        let cfg = LocalConfig::parse(r#"{"ai":{}}"#).unwrap();
        assert!(
            !cfg.ai.agent_actuator_enabled,
            "актуатор ВЫКЛ по умолчанию (safe-by-default)"
        );
        assert!(cfg.ai.agent_overwrite_threshold.is_none());
        assert!(cfg.ai.agent_blast_radius_cap.is_none());

        // Полностью пустой конфиг → тоже false.
        assert!(!LocalConfig::default().ai.agent_actuator_enabled);

        // Явный opt-in + пороги читаются.
        let on = LocalConfig::parse(
            r#"{"ai":{"agent_actuator_enabled":true,"agent_overwrite_threshold":4096,"agent_blast_radius_cap":4}}"#,
        )
        .unwrap();
        assert!(on.ai.agent_actuator_enabled);
        assert_eq!(on.ai.agent_overwrite_threshold, Some(4096));
        assert_eq!(on.ai.agent_blast_radius_cap, Some(4));
    }

    /// SKILL-2: `agent_skills_dir` по умолчанию None (агент без скиллов, без регрессии); парсится явно.
    #[test]
    fn parses_agent_skills_dir() {
        assert!(LocalConfig::parse(r#"{"ai":{}}"#)
            .unwrap()
            .ai
            .agent_skills_dir
            .is_none());
        let on =
            LocalConfig::parse(r#"{"ai":{"agent_skills_dir":"/vault/.nexus/skills"}}"#).unwrap();
        assert_eq!(
            on.ai.agent_skills_dir.as_deref(),
            Some("/vault/.nexus/skills")
        );
    }

    /// INFER-CFG: новые поля инференса. Zero-config → дефолты через геттеры (обратная совместимость);
    /// явные значения парсятся. Дефолты: first_token 300с (cold-start V100), idle 90с, connect 30с,
    /// retry 3, temperature 0.3, embedding-timeout 60с.
    #[test]
    fn infer_cfg_timeouts_defaults_and_overrides() {
        // Zero-config: chat-секция без новых полей → геттеры дают дефолты.
        let zc = LocalConfig::parse(r#"{"ai":{"chat":{"url":"http://h:8080"}}}"#).unwrap();
        let c = zc.ai.chat.unwrap();
        assert_eq!(c.first_token_timeout(), Duration::from_secs(300));
        assert_eq!(c.idle_timeout(), Duration::from_secs(90));
        assert_eq!(c.connect_timeout(), Duration::from_secs(30));
        assert_eq!(c.retry_attempts(), 3);
        assert!((c.temperature() - 0.3).abs() < f32::EPSILON);
        // Embedding zero-config → дефолтный таймаут.
        let ze = LocalConfig::parse(r#"{"ai":{"embedding":{"url":"http://h:8081"}}}"#).unwrap();
        assert_eq!(ze.ai.embedding.unwrap().timeout(), Duration::from_secs(60));

        // Явные значения (целевой 1Cat-vLLM/V100 профиль) — уважаются геттерами.
        let oc = LocalConfig::parse(
            r#"{"ai":{"chat":{"url":"http://h:8000","model":"qwen3.6-27b-awq-mtp","context_window":262144,
                 "first_token_timeout_secs":240,"idle_timeout_secs":120,"connect_timeout_secs":45,
                 "retry_attempts":1,"temperature":0.7,"reserve_output_tokens":2048},
                 "embedding":{"url":"http://h:8001","timeout_secs":180}}}"#,
        )
        .unwrap();
        let c = oc.ai.chat.unwrap();
        assert_eq!(c.first_token_timeout(), Duration::from_secs(240));
        assert_eq!(c.idle_timeout(), Duration::from_secs(120));
        assert_eq!(c.connect_timeout(), Duration::from_secs(45));
        assert_eq!(c.retry_attempts(), 1);
        assert!((c.temperature() - 0.7).abs() < f32::EPSILON);
        assert_eq!(c.reserve_output_tokens(), 2048);
        assert_eq!(c.context_window, Some(262144));
        assert_eq!(oc.ai.embedding.unwrap().timeout(), Duration::from_secs(180));
    }

    #[test]
    fn parses_fast_utility_endpoint() {
        let cfg = LocalConfig::parse(
            r#"{"ai":{"fast":{"url":"http://192.168.0.29:8084","model":"qwen"}}}"#,
        )
        .unwrap();
        let fast = cfg.ai.fast.unwrap();
        assert_eq!(fast.url, "http://192.168.0.29:8084");
        assert_eq!(fast.model.as_deref(), Some("qwen"));
        // Нет секции fast → None (fallback на gemma-fast в open_vault).
        assert!(LocalConfig::parse(r#"{"ai":{}}"#)
            .unwrap()
            .ai
            .fast
            .is_none());
    }
}
