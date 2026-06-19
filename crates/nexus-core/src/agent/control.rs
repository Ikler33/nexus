//! Персист KILL-SWITCH агента (AGENT-5) — app-local файл `agent.json` в **OS config-dir**, ЗЕРКАЛО
//! паттерна egress kill-switch (`net::persist`/`egress.json`). Осознанно НЕ в vault (синхронизированный
//! vault не должен молча менять режим автономии агента) и НЕ в keychain (режим — не секрет).
//!
//! Поле одно: `paused`. Грузится на старте (agentd restore / десктоп в будущем), пишется будущей
//! UI-кнопкой/командой (UI-1). **БЕЗ миграции БД** — это файловый флаг (как требует бриф AGENT-5).
//!
//! Fail-safe-семантика хранения: отсутствие/битый файл ⇒ дефолт `paused=false` (агент работает из
//! коробки). Сам KILL-SWITCH fail-safe в ДРУГУЮ сторону (взведён ⇒ НЕ действуем) — это инвариант
//! ПРОВЕРКИ (`run_agent_loop`/`dispatch_action`/`drive`), а дефолт ПЕРСИСТА — «не на паузе», иначе
//! свежая установка стартовала бы замороженной.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Снимок control-состояния агента, переживающий рестарт (AGENT-5). `serde(default)` → файл
/// старой/новой версии читается без паники; отсутствующее поле падает в дефолт (`paused=false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentControlState {
    /// KILL-SWITCH: `true` ⇒ агент на ПАУЗЕ (прогоны остаются queued, цикл не идёт, актуатор не пишет).
    #[serde(default)]
    pub paused: bool,
}

/// Читает состояние из `path`. Нет файла / битый JSON ⇒ дефолт (`paused=false` — агент работает из
/// коробки; деградация логируется, не валит старт).
pub fn load_control_state(path: &Path) -> AgentControlState {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "agent.json битый — control-состояние агента сброшено в дефолт");
            AgentControlState::default()
        }),
        Err(_) => AgentControlState::default(), // первого запуска файла ещё нет — норма
    }
}

/// Пишет состояние в `path` (родительский каталог создаётся). Атомарно (tmp→fsync→rename) — обрыв
/// между записью и rename не оставляет усечённый control-конфиг (как egress-персист).
pub fn save_control_state(path: &Path, state: &AgentControlState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(state).expect("AgentControlState сериализуем всегда");
    crate::vault::atomic_write_io(path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Дефолт — НЕ на паузе (агент работает из коробки).
    #[test]
    fn default_is_not_paused() {
        assert!(!AgentControlState::default().paused);
    }

    /// Состояние переживает «рестарт» (save → load), включая вложенный каталог.
    #[test]
    fn round_trips_through_config_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("agent.json");
        let state = AgentControlState { paused: true };
        save_control_state(&path, &state).unwrap();
        assert_eq!(load_control_state(&path), state);
    }

    /// Нет файла (первый запуск) и битый JSON ⇒ дефолт (paused=false), без паники (fail-safe старта).
    #[test]
    fn missing_or_corrupt_file_falls_back_to_default() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("agent.json");
        assert_eq!(load_control_state(&missing), AgentControlState::default());

        std::fs::write(&missing, "{ это не json").unwrap();
        assert_eq!(load_control_state(&missing), AgentControlState::default());
    }

    /// Forward-compat: файл от другой версии (лишние поля) читается; недостающее — дефолт.
    #[test]
    fn tolerates_unknown_and_missing_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("agent.json");
        std::fs::write(&path, r#"{ "paused": true, "future_field": 42 }"#).unwrap();
        let s = load_control_state(&path);
        assert!(s.paused);
    }

    /// Атомарность: после save() нет осиротевших tmp-файлов (запись через `atomic_write_io`).
    #[test]
    fn save_is_atomic_no_leftover_tmp() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("agent.json");
        save_control_state(&path, &AgentControlState::default()).unwrap();
        save_control_state(&path, &AgentControlState { paused: true }).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("nexus-tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "осиротевшие tmp: {leftovers:?}");
        assert!(path.exists(), "целевой файл на месте");
    }
}
