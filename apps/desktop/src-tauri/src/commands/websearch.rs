//! Команды web-агента (W-1): чтение/запись consent-конфига `websearch.json` (URL SearXNG =
//! consent на эгресс к нему) с синхронизацией политики эгресса (тоггл `Web`-фичи + "web"-allowlist).
//!
//! Сам поиск/agent-loop — срез W-2 (команда чата в web-режиме). Здесь — только consent-носитель,
//! по тому же паттерну, что `set_news_config` (NF-4).

use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::websearch::{self, WebSearchConfig};

fn config_path(app: &AppHandle) -> AppResult<std::path::PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Msg(format!("config-dir недоступен: {e}")))?;
    Ok(dir.join("websearch.json"))
}

/// Текущий конфиг web-агента (URL SearXNG + тоггл). Нет файла → дефолты (выключено).
#[tauri::command]
pub async fn get_websearch_config(app: AppHandle) -> AppResult<WebSearchConfig> {
    Ok(websearch::config::load(&config_path(&app)?))
}

/// Сохраняет конфиг (непустой URL + `enabled` = consent на эгресс к этому SearXNG, W2),
/// СИНХРОНИЗИРУЕТ политику (тоггл `Web`-фичи + "web"-allowlist — мгновенно) и возвращает применённый.
#[tauri::command]
pub async fn set_websearch_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: WebSearchConfig,
) -> AppResult<WebSearchConfig> {
    let path = config_path(&app)?;
    websearch::config::save(&path, &config)
        .map_err(|e| AppError::Msg(format!("websearch.json не записан: {e}")))?;
    websearch::config::sync_egress_policy(&state.egress_policy, &config);
    Ok(config)
}
