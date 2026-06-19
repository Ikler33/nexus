//! Локальный конфиг vault (`.nexus/local.json`, в .gitignore — ADR-002): эндпоинты/модели
//! chat и embedding. Ключи здесь НЕ в git; `*.url` валидируются анти-SSRF позже (§11).

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
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    pub url: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Размерность; если не задана — берётся из ответа модели при первом эмбеддинге.
    #[serde(default)]
    pub dim: Option<usize>,
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
