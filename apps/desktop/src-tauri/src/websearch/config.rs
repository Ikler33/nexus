//! Конфиг web-агента (W2): app-local `websearch.json` в **OS config-dir** — рядом с `news.json`/
//! `egress.json` и по той же причине ВНЕ vault/git (сохранение URL SearXNG = сетевой consent, он
//! не должен приезжать git-pull'ом молча) и вне keychain (URL не секрет).
//!
//! Дефолты fail-safe: фича ВЫКЛЮЧЕНА, URL пуст. Сохранение непустого URL = явный consent (W2):
//! вызывающий (команда) включает фичу и кладёт хост в allowlist скоупа "web".

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Пользовательский конфиг web-агента — переживает рестарт, редактируется в настройках.
/// `Default` = fail-safe: `enabled=false`, пустой URL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchConfig {
    /// Тоггл фичи (= consent на эгресс к SearXNG, W2). По умолчанию ВЫКЛ.
    #[serde(default)]
    pub enabled: bool,
    /// URL инстанса SearXNG (напр. `https://searx.example.com`). Пустой → фича не работает.
    #[serde(default)]
    pub url: String,
}

impl WebSearchConfig {
    /// Хост из URL (для allowlist скоупа "web") — `None`, если URL пуст/без хоста.
    pub fn host(&self) -> Option<String> {
        let u = self.url.trim();
        if u.is_empty() {
            return None;
        }
        reqwest::Url::parse(u)
            .ok()
            .and_then(|p| p.host_str().map(str::to_string))
    }

    /// Действует ли фича: включена И есть валидный хост (consent на конкретный SearXNG).
    pub fn is_active(&self) -> bool {
        self.enabled && self.host().is_some()
    }
}

/// Синхронизирует политику эгресса с конфигом web-агента (W2): тоггл `Web`-фичи + "web"-скоуп
/// allowlist (хост SearXNG из URL — мгновенно). Выключено/нет URL → пустой allowlist (fail-closed).
pub fn sync_egress_policy(policy: &crate::net::EgressPolicy, cfg: &WebSearchConfig) {
    let active = cfg.is_active();
    policy.set_feature_enabled(crate::net::EgressFeature::Web, active);
    let hosts: Vec<String> = if active {
        cfg.host().into_iter().collect()
    } else {
        Vec::new()
    };
    policy.set_scoped_allowlist("web", hosts);
}

/// Читает конфиг; нет файла/битый JSON → дефолты (фича выключена — fail-safe, W2).
pub fn load(path: &Path) -> WebSearchConfig {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "websearch.json битый — конфиг web-агента сброшен в дефолты");
            WebSearchConfig::default()
        }),
        Err(_) => WebSearchConfig::default(),
    }
}

/// Пишет конфиг (каталог создаётся).
pub fn save(path: &Path, cfg: &WebSearchConfig) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(cfg).expect("WebSearchConfig сериализуем всегда");
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn defaults_are_fail_safe() {
        let cfg = WebSearchConfig::default();
        assert!(!cfg.enabled, "consent не из коробки (W2)");
        assert!(cfg.url.is_empty());
        assert!(!cfg.is_active());
        assert_eq!(cfg.host(), None);
    }

    #[test]
    fn host_extracted_and_active_requires_both() {
        let cfg = WebSearchConfig {
            enabled: true,
            url: "https://searx.example.com/search".into(),
        };
        assert_eq!(cfg.host().as_deref(), Some("searx.example.com"));
        assert!(cfg.is_active());

        // Включена, но URL пуст → не активна (нет хоста для consent).
        let no_url = WebSearchConfig {
            enabled: true,
            url: "  ".into(),
        };
        assert!(!no_url.is_active());

        // URL есть, но выключена → не активна.
        let off = WebSearchConfig {
            enabled: false,
            url: "https://searx.example.com".into(),
        };
        assert!(!off.is_active());
    }

    #[test]
    fn sync_egress_policy_toggles_feature_and_allowlist() {
        use crate::net::{EgressFeature, EgressPolicy};
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let policy = EgressPolicy::new(Arc::new(AtomicBool::new(false)));
        // Активный consent → Web включена, хост в "web"-allowlist (публичный хост проходит check).
        let cfg = WebSearchConfig {
            enabled: true,
            url: "https://searx.example.com".into(),
        };
        sync_egress_policy(&policy, &cfg);
        assert!(policy.is_feature_enabled(EgressFeature::Web));
        assert!(policy
            .check("searx.example.com", EgressFeature::Web)
            .is_ok());

        // Выключение → фича off, allowlist пуст (fail-closed).
        let off = WebSearchConfig {
            enabled: false,
            url: "https://searx.example.com".into(),
        };
        sync_egress_policy(&policy, &off);
        assert!(!policy.is_feature_enabled(EgressFeature::Web));
        assert!(policy
            .check("searx.example.com", EgressFeature::Web)
            .is_err());
    }

    #[test]
    fn roundtrip_save_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("websearch.json");
        let cfg = WebSearchConfig {
            enabled: true,
            url: "https://searx.example.com".into(),
        };
        save(&path, &cfg).unwrap();
        assert_eq!(load(&path), cfg);

        // Нет файла → дефолты.
        let missing = dir.path().join("nope.json");
        assert_eq!(load(&missing), WebSearchConfig::default());
    }
}
