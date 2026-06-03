//! Команды git-sync (Фаза 3, §8): статус и коммит vault. Все libgit2-операции синхронны → выполняются
//! в `spawn_blocking`, под sync-локом `AppState::git_lock` (один синк/коммит за раз). git-sync —
//! core module (не sandbox-плагин). Репозиторий открывается/инициируется per-вызов (git2 `!Send`).

use tauri::State;

use crate::git::{CommitOutcome, GitSync, StatusEntry};
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
