//! Персист политики эгресса (E5, срез 2 «UI/контроль» `net.md`): app-local файл `egress.json`
//! в **OS config-dir** — осознанно НЕ в vault (`.nexus/local.json` приходит через git-pull, С-18:
//! синхронизированный vault не должен молча расширять сетевую границу) и НЕ в keychain (политика —
//! не секрет). Грузится на старте приложения (setup-хук), пишется командами `set_egress_*`.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Снимок политики эгресса — то, что переживает рестарт (E5) и что видит UI настроек (срез 2).
/// Все поля — `serde(default)`: файл от старой/новой версии приложения читается без паники,
/// отсутствующее поле падает в local-first-дефолт (fail-safe: фичи включены, офлайн выключен).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressState {
    /// Kill-switch «офлайн» (E2): публичные хосты отрезаны, LAN/loopback живут.
    #[serde(default)]
    pub offline: bool,
    /// Per-feature opt-in (E6); local-first — по умолчанию включены.
    #[serde(default = "default_on")]
    pub chat: bool,
    #[serde(default = "default_on")]
    pub embed: bool,
    #[serde(default = "default_on")]
    pub probe: bool,
}

fn default_on() -> bool {
    true
}

impl Default for EgressState {
    fn default() -> Self {
        Self {
            offline: false,
            chat: true,
            embed: true,
            probe: true,
        }
    }
}

/// Читает состояние из `path`. Нет файла / битый JSON → дефолты (local-first работает из коробки;
/// «no silent caps» — деградация логируется, но не валит старт).
pub fn load(path: &Path) -> EgressState {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "egress.json битый — политика эгресса сброшена в дефолты");
            EgressState::default()
        }),
        Err(_) => EgressState::default(), // первого запуска файла ещё нет — норма
    }
}

/// Пишет состояние в `path` (родительский каталог создаётся). Файл крошечный — обычная запись.
pub fn save(path: &Path, state: &EgressState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(state).expect("EgressState сериализуем всегда");
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// E5: состояние переживает «рестарт» (save → load), включая вложенный каталог.
    #[test]
    fn round_trips_through_config_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("egress.json");
        let state = EgressState {
            offline: true,
            chat: true,
            embed: false,
            probe: true,
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
    }

    /// Нет файла (первый запуск) и битый JSON → local-first-дефолты, без паники (fail-safe).
    #[test]
    fn missing_or_corrupt_file_falls_back_to_defaults() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("egress.json");
        assert_eq!(load(&missing), EgressState::default());

        std::fs::write(&missing, "{ это не json").unwrap();
        assert_eq!(load(&missing), EgressState::default());
    }

    /// Forward-compat: файл от другой версии (лишние/недостающие поля) читается; недостающее —
    /// в local-first-дефолт (фичи on), известное — сохраняется.
    #[test]
    fn tolerates_unknown_and_missing_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("egress.json");
        std::fs::write(&path, r#"{ "offline": true, "future_field": 42 }"#).unwrap();
        let s = load(&path);
        assert!(s.offline);
        assert!(
            s.chat && s.embed && s.probe,
            "недостающие фичи → default on"
        );
    }
}
