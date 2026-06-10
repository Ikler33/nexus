//! Команды политики эгресса (срез 2 «UI/контроль» `net.md`): чтение состояния для настроек,
//! тоггл «офлайн» (E2) и per-feature opt-in (E6). Каждое изменение персистится в OS config-dir
//! (E5, `net::persist`) — политика переживает рестарт. Бэкенд-половина среза; UI-тоггл и
//! чат-бейдж (E9) — фронт-срезом.

use std::str::FromStr;

use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::net::{self, EgressFeature, EgressState};
use crate::state::AppState;

/// Текущее состояние политики эгресса — для префилла настроек (и будущего бейджа E9).
#[tauri::command]
pub async fn get_egress_state(state: State<'_, AppState>) -> AppResult<EgressState> {
    Ok(state.egress_state())
}

/// Тоггл «офлайн» (E2/AC-EGR-3): публичные хосты отрезаются, LAN/loopback живут. Включение
/// дорезает активный chat-стрим через существующий `chat_cancel` (E10/AC-EGR-11). Персистится.
#[tauri::command]
pub async fn set_egress_offline(
    app: AppHandle,
    state: State<'_, AppState>,
    offline: bool,
) -> AppResult<EgressState> {
    state.set_egress_offline(offline);
    persist(&app, &state)
}

/// Per-feature opt-in (E6/AC-EGR-5): `feature` — `chat` | `embed` | `probe`. Персистится.
#[tauri::command]
pub async fn set_egress_feature(
    app: AppHandle,
    state: State<'_, AppState>,
    feature: String,
    enabled: bool,
) -> AppResult<EgressState> {
    let f = EgressFeature::from_str(&feature)
        .map_err(|()| AppError::Msg(format!("неизвестная сетевая фича: {feature}")))?;
    state.egress_policy.set_feature_enabled(f, enabled);
    persist(&app, &state)
}

/// Пишет текущее состояние в `<OS config-dir>/egress.json` (E5) и возвращает его (фронт обновляет
/// форму одним ответом). Ошибка записи — видимая (no silent caps): состояние применено в памяти,
/// но рестарт его потеряет — UI должен показать ошибку.
fn persist(app: &AppHandle, state: &State<'_, AppState>) -> AppResult<EgressState> {
    let snapshot = state.egress_state();
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Msg(format!("config-dir недоступен: {e}")))?;
    net::save_egress_state(&dir.join("egress.json"), &snapshot)
        .map_err(|e| AppError::Msg(format!("egress.json не записан: {e}")))?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Снимок ↔ применение состояния через AppState: тоггл фич и офлайна виден политике
    /// per-request; применение сохранённого офлайна взводит существующий chat_cancel (E10).
    #[test]
    fn state_snapshot_and_apply_round_trip() {
        let state = AppState::new();
        let token = state.begin_chat();

        let saved = EgressState {
            offline: true,
            chat: true,
            embed: false,
            probe: true,
        };
        state.apply_egress_state(&saved);

        assert_eq!(state.egress_state(), saved, "снимок отражает применённое");
        assert!(
            !state.egress_policy.is_feature_enabled(EgressFeature::Embed),
            "политика видит выключенный embed per-request"
        );
        assert!(
            token.load(std::sync::atomic::Ordering::Relaxed),
            "применение offline=true дорезает активный стрим (единый инвариант E10)"
        );
    }

    /// Парсер фичи — обратное к Display; неизвестное имя отклоняется (команда вернёт ошибку).
    #[test]
    fn feature_parses_from_frontend_string() {
        assert_eq!(EgressFeature::from_str("chat"), Ok(EgressFeature::Chat));
        assert_eq!(EgressFeature::from_str("embed"), Ok(EgressFeature::Embed));
        assert_eq!(EgressFeature::from_str("probe"), Ok(EgressFeature::Probe));
        assert!(
            EgressFeature::from_str("web").is_err(),
            "Web — срез 4, не сейчас"
        );
    }
}
