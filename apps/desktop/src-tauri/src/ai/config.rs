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
    /// Chat-провайдер (Qwen3 и т.п.) — отдельный хост (ADR-005).
    pub chat: Option<ChatConfig>,
    /// Embedding-провайдер (мультиязычный) — отдельный хост (ADR-005).
    pub embedding: Option<EmbeddingConfig>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_json() {
        // Форма из ARCHITECTURE §5 (.nexus/local.json).
        let json = r#"{
          "ai": {
            "chat":      { "url": "http://192.168.0.172:8080", "model": "qwen3", "context_window": 32768 },
            "embedding": { "url": "http://127.0.0.1:8081", "model": "nomic-embed-text", "dim": 768 },
            "reranker":  { "url": "http://127.0.0.1:8082", "enabled": false }
          },
          "sync": { "remote": null }
        }"#;
        let cfg = LocalConfig::parse(json).unwrap();
        let chat = cfg.ai.chat.unwrap();
        assert_eq!(chat.url, "http://192.168.0.172:8080");
        assert_eq!(chat.context_window, Some(32768));
        let emb = cfg.ai.embedding.unwrap();
        assert_eq!(emb.url, "http://127.0.0.1:8081");
        assert_eq!(emb.dim, Some(768));
    }

    #[test]
    fn tolerates_partial_and_unknown_fields() {
        let cfg = LocalConfig::parse(r#"{"ai":{"embedding":{"url":"http://x:8081"}}}"#).unwrap();
        assert!(cfg.ai.chat.is_none());
        assert_eq!(cfg.ai.embedding.unwrap().dim, None);
    }
}
