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
