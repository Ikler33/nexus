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

/// Закрывает сессию плагина: мгновенно отзывает токен в брокере (§7.9). Вызывается фронтом при
/// размонтировании плагина (закрытие панели/iframe) — иначе сессии копятся в брокере. Идемпотентно.
#[tauri::command]
pub async fn plugin_close_session(state: State<'_, AppState>, token: String) -> Result<(), String> {
    let token = CapToken::from_ipc(token);
    state
        .plugins
        .lock()
        .map_err(|_| "broker lock")?
        .revoke(&token);
    Ok(())
}

/// Host-функция плагина через брокер (§7.4): авторизация по токену + scoped-проверка + audit, затем
/// dispatch. Поддержаны `vault.readFile`/`vault.listFiles` (право `vault:read`) и `vault.writeFile`
/// (`vault:write`). Результат — JSON: строка-контент / массив записей каталога / `{ok,bytes}`.
#[tauri::command]
pub async fn plugin_invoke(
    state: State<'_, AppState>,
    token: String,
    method: String,
    path: Option<String>,
    content: Option<String>,
) -> Result<serde_json::Value, String> {
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

    // Реальный I/O — вне лока, через тестируемый dispatch.
    dispatch_vault(&vault_root, &method, path.as_deref(), content.as_deref()).await
}

/// Реальный I/O авторизованного vault-вызова. Отдельная (тестируемая) функция: брокер уже проверил
/// право+scope, здесь — резолв пути (та же анти-traversal граница, defense-in-depth) и сам I/O.
/// `vault.onFileChanged` — подписка на события, не invoke-метод (придёт отдельным каналом).
async fn dispatch_vault(
    vault_root: &Path,
    method: &str,
    path: Option<&str>,
    content: Option<&str>,
) -> Result<serde_json::Value, String> {
    match method {
        "vault.readFile" => {
            let p = path.ok_or("нет аргумента path")?;
            let abs =
                vault::resolve_vault_path(vault_root, Path::new(p)).map_err(|e| e.to_string())?;
            let text = tokio::fs::read_to_string(&abs)
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::String(text))
        }
        "vault.listFiles" => {
            // Пустой путь = корень vault. `list_dir` сам резолвит rel (та же граница), скрывает
            // служебное/dotfiles, не рекурсивен. Синхронный I/O → spawn_blocking.
            let root = vault_root.to_path_buf();
            let rel = std::path::PathBuf::from(path.unwrap_or(""));
            let entries = tokio::task::spawn_blocking(move || vault::list_dir(&root, &rel))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            serde_json::to_value(entries).map_err(|e| e.to_string())
        }
        "vault.writeFile" => {
            let p = path.ok_or("нет аргумента path")?;
            let body = content.ok_or("нет аргумента content")?;
            // Запись: канонизируем РОДИТЕЛЯ (файл может не существовать) — `*_for_write`.
            let abs = vault::resolve_vault_path_for_write(vault_root, Path::new(p))
                .map_err(|e| e.to_string())?;
            tokio::fs::write(&abs, body)
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "ok": true, "bytes": body.len() }))
        }
        other => Err(format!("метод пока не поддержан host-стороной: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{Permissions, PluginBroker};
    use tempfile::TempDir;

    fn vault() -> TempDir {
        let d = TempDir::new().unwrap();
        std::fs::create_dir(d.path().join("Notes")).unwrap();
        std::fs::write(d.path().join("Notes/a.md"), "# A\nтекст").unwrap();
        d
    }

    #[tokio::test]
    async fn dispatch_reads_lists_writes_within_vault() {
        let v = vault();
        let root = v.path().canonicalize().unwrap();

        let r = dispatch_vault(&root, "vault.readFile", Some("Notes/a.md"), None)
            .await
            .unwrap();
        assert_eq!(r.as_str().unwrap(), "# A\nтекст");

        let r = dispatch_vault(&root, "vault.listFiles", Some(""), None)
            .await
            .unwrap();
        let names: Vec<&str> = r
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"Notes"));

        let r = dispatch_vault(&root, "vault.writeFile", Some("Notes/new.md"), Some("hi"))
            .await
            .unwrap();
        assert_eq!(r["ok"], serde_json::json!(true));
        assert_eq!(
            std::fs::read_to_string(root.join("Notes/new.md")).unwrap(),
            "hi"
        );
    }

    #[tokio::test]
    async fn dispatch_blocks_path_escape() {
        let v = vault();
        let root = v.path().canonicalize().unwrap();
        assert!(
            dispatch_vault(&root, "vault.readFile", Some("../../etc/passwd"), None)
                .await
                .is_err()
        );
        assert!(
            dispatch_vault(&root, "vault.readFile", Some("/etc/passwd"), None)
                .await
                .is_err()
        );
        assert!(
            dispatch_vault(&root, "vault.writeFile", Some("../evil.md"), Some("x"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn dispatch_unknown_method_and_missing_args() {
        let v = vault();
        let root = v.path().canonicalize().unwrap();
        assert!(
            dispatch_vault(&root, "vault.deleteEverything", Some("x"), None)
                .await
                .is_err()
        );
        assert!(dispatch_vault(&root, "vault.readFile", None, None)
            .await
            .is_err());
        assert!(dispatch_vault(&root, "vault.writeFile", Some("a.md"), None)
            .await
            .is_err());
    }

    /// E2E-логика `plugin_invoke` без Tauri: брокер авторизует scope → dispatch делает I/O.
    /// Проверяет связку «scope (broker) + boundary+I/O (dispatch)» и аудит (allow+deny).
    #[tokio::test]
    async fn broker_scope_then_dispatch_end_to_end() {
        async fn invoke(
            broker: &mut PluginBroker,
            token: &CapToken,
            root: &Path,
            method: &str,
            path: Option<&str>,
            content: Option<&str>,
        ) -> Result<serde_json::Value, String> {
            let req = ApiRequest {
                method,
                path,
                host: None,
            };
            broker.authorize(token, &req).map_err(|e| e.to_string())?;
            dispatch_vault(root, method, path, content).await
        }

        let v = vault();
        let root = v.path().canonicalize().unwrap();
        let mut broker = PluginBroker::new();
        let permissions: Permissions =
            serde_json::from_str(r#"{"vault:read":["Notes/**"],"vault:write":["Notes/**"]}"#)
                .unwrap();
        let token = broker.open_session(PluginSession {
            id: "demo".into(),
            permissions,
            vault_root: root.clone(),
        });

        // read/write в scope — ок; вне scope — брокер отказывает ДО I/O.
        assert!(invoke(
            &mut broker,
            &token,
            &root,
            "vault.readFile",
            Some("Notes/a.md"),
            None
        )
        .await
        .is_ok());
        assert!(invoke(
            &mut broker,
            &token,
            &root,
            "vault.readFile",
            Some("Secrets/x.md"),
            None
        )
        .await
        .is_err());
        assert!(invoke(
            &mut broker,
            &token,
            &root,
            "vault.writeFile",
            Some("Notes/b.md"),
            Some("hi")
        )
        .await
        .is_ok());
        assert!(invoke(
            &mut broker,
            &token,
            &root,
            "vault.writeFile",
            Some("Secrets/b.md"),
            Some("hi")
        )
        .await
        .is_err());

        let audit = broker.audit().entries();
        assert!(audit.iter().any(|e| e.allowed));
        assert!(audit.iter().any(|e| !e.allowed));
    }
}
