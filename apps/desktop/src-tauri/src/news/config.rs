//! Конфиг ленты (NF-3, спека D2/D7): app-local `news.json` в **OS config-dir** — рядом с
//! `egress.json` и по той же причине ВНЕ vault/git (включение фичи и добавление источника =
//! сетевой consent, он не должен приезжать git-pull'ом молча) и вне keychain (не секрет).
//!
//! Дефолты fail-safe: фича ВЫКЛЮЧЕНА (web-класс не из коробки, E4/W2 — consent при включении),
//! ключи — пресет [`super::DEFAULT_KEYWORDS`], источники — реестр v1 со своими
//! `default_enabled`-флагами (arxiv выключен).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Source, DEFAULT_KEYWORDS, SOURCES_V1};

/// Пользовательский конфиг ленты — то, что переживает рестарт и редактируется со страницы.
/// `Default` (derive) = fail-safe: `enabled=false`, пустые переопределения, пресет ключей.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsConfig {
    /// Тоггл фичи (= consent на эгресс к источникам, AC-NF-7). По умолчанию ВЫКЛ.
    #[serde(default)]
    pub enabled: bool,
    /// Переопределения вкл/выкл источников реестра: id → bool (нет записи → `default_enabled`).
    #[serde(default)]
    pub sources: std::collections::BTreeMap<String, bool>,
    /// Ключевые слова этапа 1 (D2). `None` → пресет (отличаем «не трогал» от «очистил сам»).
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    /// Доп. хосты статей, разрешённые владельцем по клику из ридера (opt-in 2026-06-11): статья
    /// агрегатора (HN и т.п.) может жить вне хостов источников — каждый хост разрешается ЯВНО и
    /// по одному, хранится здесь (вне vault/git, как весь consent), снимается из gear-меню ленты.
    /// Класс защиты НЕ ослабляется: web-класс по-прежнему режет приватные/LAN (+DNS-гард с пином),
    /// капы/таймауты/маркеры те же — consent добавляет только ПУБЛИЧНЫЙ хост в "news"-скоуп.
    #[serde(default)]
    pub extra_hosts: Vec<String>,
}

impl NewsConfig {
    /// Действующие ключевые слова: пользовательские или пресет.
    pub fn effective_keywords(&self) -> Vec<String> {
        match &self.keywords {
            Some(k) => k.clone(),
            None => DEFAULT_KEYWORDS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Включён ли источник реестра с учётом переопределений.
    pub fn source_enabled(&self, s: &Source) -> bool {
        self.sources.get(s.id).copied().unwrap_or(s.default_enabled)
    }

    /// Активные источники прогона (фича включена проверяется вызывающим).
    pub fn active_sources(&self) -> Vec<&'static Source> {
        SOURCES_V1
            .iter()
            .filter(|s| self.source_enabled(s))
            .collect()
    }
}

/// Читает конфиг; нет файла/битый JSON → дефолты (фича выключена — fail-safe, AC-NF-7).
pub fn load(path: &Path) -> NewsConfig {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "news.json битый — конфиг ленты сброшен в дефолты");
            NewsConfig::default()
        }),
        Err(_) => NewsConfig::default(),
    }
}

/// Пишет конфиг (каталог создаётся).
pub fn save(path: &Path, cfg: &NewsConfig) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(cfg).expect("NewsConfig сериализуем всегда");
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Дефолты fail-safe: фича ВЫКЛ, ключи — пресет, arxiv выключен, остальной реестр включён.
    #[test]
    fn defaults_are_fail_safe() {
        let cfg = NewsConfig::default();
        assert!(!cfg.enabled, "consent не из коробки (AC-NF-7)");
        assert_eq!(cfg.effective_keywords().len(), DEFAULT_KEYWORDS.len());
        let active = cfg.active_sources();
        assert!(
            active.iter().all(|s| !s.id.starts_with("arxiv")),
            "arxiv выключен (D1)"
        );
        assert!(active.iter().any(|s| s.id == "openai"));
    }

    /// Переопределения: выключить дефолтный источник, включить arxiv; свои ключи в силе.
    #[test]
    fn overrides_apply() {
        let mut cfg = NewsConfig::default();
        cfg.sources.insert("openai".into(), false);
        cfg.sources.insert("arxiv-cs-ai".into(), true);
        cfg.keywords = Some(vec!["mcp".into()]);

        let ids: Vec<_> = cfg.active_sources().iter().map(|s| s.id).collect();
        assert!(!ids.contains(&"openai"));
        assert!(ids.contains(&"arxiv-cs-ai"));
        assert_eq!(cfg.effective_keywords(), vec!["mcp".to_string()]);
    }

    /// Персист: round-trip; битый/отсутствующий файл → дефолты (фича выключена).
    #[test]
    fn persists_and_falls_back() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("news.json");
        let mut cfg = NewsConfig {
            enabled: true,
            ..Default::default()
        };
        cfg.sources.insert("hn".into(), false);
        cfg.extra_hosts = vec!["example.com".into()];
        save(&path, &cfg).unwrap();
        assert_eq!(load(&path), cfg);

        std::fs::write(&path, "{оборвано").unwrap();
        assert!(!load(&path).enabled, "битый файл → fail-safe выкл");
        assert!(!load(&dir.path().join("нет.json")).enabled);
    }
}
