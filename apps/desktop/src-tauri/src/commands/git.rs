//! Команды git-sync (Фаза 3, §8): статус и коммит vault. Все libgit2-операции синхронны → выполняются
//! в `spawn_blocking`, под sync-локом `AppState::git_lock` (один синк/коммит за раз). git-sync —
//! core module (не sandbox-плагин). Репозиторий открывается/инициируется per-вызов (git2 `!Send`).

use tauri::State;

use crate::git::{creds, CommitOutcome, GitSync, PullOutcome, StatusEntry};
use crate::state::AppState;

/// Корень открытого vault или ошибка «vault не открыт».
async fn vault_root(state: &State<'_, AppState>) -> Result<std::path::PathBuf, String> {
    let guard = state.vault.read().await;
    Ok(guard.as_ref().ok_or("vault не открыт")?.root.clone())
}

/// Статус рабочего дерева vault: изменённые/новые/удалённые (без игнорируемых). Открывает/инициирует
/// репозиторий и гарантирует управляемый `.gitignore` (исключает `.nexus/` — секреты/код плагинов).
#[tauri::command]
pub async fn git_status(state: State<'_, AppState>) -> Result<Vec<StatusEntry>, String> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await; // sync-lock: один git-вызов за раз
    tokio::task::spawn_blocking(move || -> Result<Vec<StatusEntry>, String> {
        let git = GitSync::open_or_init(&root).map_err(|e| e.to_string())?;
        git.ensure_gitignore().map_err(|e| e.to_string())?;
        git.status().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Авто-коммит изменений vault: secret-scan → при находке коммит НЕ делается (`blocked-by-secrets`),
/// иначе коммит с авто-сообщением. Возвращает исход (`CommitOutcome`).
#[tauri::command]
pub async fn git_commit(state: State<'_, AppState>) -> Result<CommitOutcome, String> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> Result<CommitOutcome, String> {
        let git = GitSync::open_or_init(&root).map_err(|e| e.to_string())?;
        git.ensure_gitignore().map_err(|e| e.to_string())?;
        git.commit_all().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Сохранить токен доступа к remote в системном keychain (Ф3-3b, AC-SEC-3): на диск НЕ пишется.
/// Аккаунт записи — путь vault (разные vault → разные токены). keychain-I/O синхронный → spawn_blocking.
#[tauri::command]
pub async fn git_set_token(state: State<'_, AppState>, token: String) -> Result<(), String> {
    let account = vault_root(&state).await?.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || {
        creds::set_token(&account, &token).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Удалить токен доступа из keychain (идемпотентно).
#[tauri::command]
pub async fn git_clear_token(state: State<'_, AppState>) -> Result<(), String> {
    let account = vault_root(&state).await?.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || creds::delete_token(&account).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Есть ли сохранённый токен для текущего vault (для UI: показать «подключено»).
#[tauri::command]
pub async fn git_has_token(state: State<'_, AppState>) -> Result<bool, String> {
    let account = vault_root(&state).await?.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || creds::has_token(&account).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Устанавливает URL remote `origin`.
#[tauri::command]
pub async fn git_set_remote(state: State<'_, AppState>, url: String) -> Result<(), String> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        GitSync::open_or_init(&root)
            .map_err(|e| e.to_string())?
            .set_remote(&url)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// URL remote `origin` (если задан).
#[tauri::command]
pub async fn git_get_remote(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
        GitSync::open_or_init(&root)
            .map_err(|e| e.to_string())?
            .get_remote()
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Синхронизация с remote: pull (fast-forward) → push (если не конфликт). Токен берётся из keychain.
/// `MergeRequired` (расхождение истории) → НЕ пушим, сигналим для ручного разрешения (Ф3-3b-3).
#[tauri::command]
pub async fn git_sync(state: State<'_, AppState>) -> Result<PullOutcome, String> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> Result<PullOutcome, String> {
        let account = root.to_string_lossy();
        let token = creds::get_token(&account)
            .map_err(|e| e.to_string())?
            .ok_or("нет токена доступа — сохрани его (keychain)")?;
        let git = GitSync::open_or_init(&root).map_err(|e| e.to_string())?;
        let pulled = git.pull(&token).map_err(|e| e.to_string())?;
        if pulled != PullOutcome::MergeRequired {
            git.push(&token).map_err(|e| e.to_string())?;
        }
        Ok(pulled)
    })
    .await
    .map_err(|e| e.to_string())?
}
