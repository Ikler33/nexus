//! Команды плагинов: Ф0-13 (манифесты + совместимость) + Ф2-2b (capability-broker live):
//! `plugin_open_session` (выдать токен по правам манифеста) и `plugin_invoke` (host-функция через
//! брокер: авторизация по токену + scoped-проверка + audit → dispatch). §7.4/§7.9, ADR-002.

use std::path::Path;

use tauri::State;

use crate::plugin::{self, ApiRequest, CapToken, PluginInfo, PluginSession};
use crate::state::AppState;
use crate::vault;

/// Список установленных плагинов vault (`.nexus/plugins/*`) с их статусом совместимости.
#[tauri::command]
pub async fn list_plugins(state: State<'_, AppState>) -> Result<Vec<PluginInfo>, String> {
    let root = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.root.clone()
    };
    let dir = root.join(".nexus").join("plugins");
    tokio::task::spawn_blocking(move || plugin::scan_plugins(&dir))
        .await
        .map_err(|e| e.to_string())
}

/// Открывает сессию плагина (`.nexus/plugins/<dir>`): читает манифест, проверяет совместимость,
/// заводит сессию с его scoped-правами в брокере и возвращает **capability-токен** (§7.9). Фронт
/// передаёт токен с каждым `plugin_invoke`. Несовместимый/битый манифест → ошибка (не загружаем).
#[tauri::command]
pub async fn plugin_open_session(
    state: State<'_, AppState>,
    dir: String,
) -> Result<String, String> {
    let root = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.root.clone()
    };
    let manifest_path = root
        .join(".nexus")
        .join("plugins")
        .join(&dir)
        .join("manifest.json");
    let json = tokio::fs::read_to_string(&manifest_path)
        .await
        .map_err(|e| format!("manifest: {e}"))?;
    let manifest =
        plugin::load_manifest(&json, plugin::CORE_API_VERSION).map_err(|e| e.to_string())?;

    let session = PluginSession {
        id: manifest.id,
        permissions: manifest.permissions,
        vault_root: root,
    };
    let token = state
        .plugins
        .lock()
        .map_err(|_| "broker lock")?
        .open_session(session);
    Ok(token.as_str().to_string())
}

/// Host-функция плагина через брокер (§7.4): авторизация по токену + scoped-проверка + audit, затем
/// dispatch. Сейчас поддержан `vault.readFile` (read-only); write/list/ai — следующими срезами.
#[tauri::command]
pub async fn plugin_invoke(
    state: State<'_, AppState>,
    token: String,
    method: String,
    path: Option<String>,
) -> Result<String, String> {
    let token = CapToken::from_ipc(token);

    // Авторизация (синхронно, под локом) → достаём vault_root, лок отпускаем до async I/O.
    let vault_root = {
        let mut broker = state.plugins.lock().map_err(|_| "broker lock")?;
        let req = ApiRequest {
            method: &method,
            path: path.as_deref(),
            host: None,
        };
        broker.authorize(&token, &req).map_err(|e| e.to_string())?;
        broker
            .session(&token)
            .ok_or("сессия не найдена")?
            .vault_root
            .clone()
    };

    // Dispatch авторизованного вызова (реальный I/O вне лока).
    match method.as_str() {
        "vault.readFile" => {
            let p = path.ok_or("нет аргумента path")?;
            let abs =
                vault::resolve_vault_path(&vault_root, Path::new(&p)).map_err(|e| e.to_string())?;
            tokio::fs::read_to_string(&abs)
                .await
                .map_err(|e| e.to_string())
        }
        other => Err(format!("метод пока не поддержан host-стороной: {other}")),
    }
}
