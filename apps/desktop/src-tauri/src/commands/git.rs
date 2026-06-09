//! Команды git-sync (Фаза 3, §8): статус и коммит vault. Все libgit2-операции синхронны → выполняются
//! в `spawn_blocking`, под sync-локом `AppState::git_lock` (один синк/коммит за раз). git-sync —
//! core module (не sandbox-плагин). Репозиторий открывается/инициируется per-вызов (git2 `!Send`).

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::git::{creds, CommitOutcome, GitSync, MergePreview, PullOutcome, StatusEntry};
use crate::state::AppState;

/// Корень открытого vault или [`AppError::NoVault`].
async fn vault_root(state: &State<'_, AppState>) -> AppResult<std::path::PathBuf> {
    Ok(state.vault().await?.root.clone())
}

/// JoinError из `spawn_blocking` → понятная ad-hoc ошибка (паника/отмена blocking-задачи).
fn join_err(e: tokio::task::JoinError) -> AppError {
    AppError::Msg(format!("git: фоновая задача прервана: {e}"))
}

/// Статус рабочего дерева vault: изменённые/новые/удалённые (без игнорируемых). Открывает/инициирует
/// репозиторий и гарантирует управляемый `.gitignore` (исключает `.nexus/` — секреты/код плагинов).
#[tauri::command]
pub async fn git_status(state: State<'_, AppState>) -> AppResult<Vec<StatusEntry>> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await; // sync-lock: один git-вызов за раз
    tokio::task::spawn_blocking(move || -> AppResult<Vec<StatusEntry>> {
        let git = GitSync::open_or_init(&root)?;
        git.ensure_gitignore()?;
        Ok(git.status()?)
    })
    .await
    .map_err(join_err)?
}

/// Авто-коммит изменений vault: secret-scan → при находке коммит НЕ делается (`blocked-by-secrets`),
/// иначе коммит с авто-сообщением. Возвращает исход (`CommitOutcome`).
#[tauri::command]
pub async fn git_commit(state: State<'_, AppState>) -> AppResult<CommitOutcome> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> AppResult<CommitOutcome> {
        let git = GitSync::open_or_init(&root)?;
        git.ensure_gitignore()?;
        Ok(git.commit_all()?)
    })
    .await
    .map_err(join_err)?
}

/// Выборочный коммит (#10): коммитит ТОЛЬКО переданные пути (из `git_status`), а не всё-или-ничего.
/// Secret-scan по коммитимым файлам; устаревший/пустой выбор → `nothing-to-commit`. Под sync-локом.
#[tauri::command]
pub async fn git_commit_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> AppResult<CommitOutcome> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> AppResult<CommitOutcome> {
        let git = GitSync::open_or_init(&root)?;
        git.ensure_gitignore()?;
        Ok(git.commit_paths(&paths)?)
    })
    .await
    .map_err(join_err)?
}

/// Сохранить токен доступа к remote в системном keychain (Ф3-3b, AC-SEC-3): на диск НЕ пишется.
/// Аккаунт записи — путь vault (разные vault → разные токены). keychain-I/O синхронный → spawn_blocking.
#[tauri::command]
pub async fn git_set_token(state: State<'_, AppState>, token: String) -> AppResult<()> {
    let account = vault_root(&state).await?.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        Ok(creds::set_token(&account, &token)?)
    })
    .await
    .map_err(join_err)?
}

/// Удалить токен доступа из keychain (идемпотентно).
#[tauri::command]
pub async fn git_clear_token(state: State<'_, AppState>) -> AppResult<()> {
    let account = vault_root(&state).await?.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || -> AppResult<()> { Ok(creds::delete_token(&account)?) })
        .await
        .map_err(join_err)?
}

/// Есть ли сохранённый токен для текущего vault (для UI: показать «подключено»).
#[tauri::command]
pub async fn git_has_token(state: State<'_, AppState>) -> AppResult<bool> {
    let account = vault_root(&state).await?.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || -> AppResult<bool> { Ok(creds::has_token(&account)?) })
        .await
        .map_err(join_err)?
}

/// Устанавливает URL remote `origin`.
#[tauri::command]
pub async fn git_set_remote(state: State<'_, AppState>, url: String) -> AppResult<()> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        GitSync::open_or_init(&root)?.set_remote(&url)?;
        Ok(())
    })
    .await
    .map_err(join_err)?
}

/// URL remote `origin` (если задан).
#[tauri::command]
pub async fn git_get_remote(state: State<'_, AppState>) -> AppResult<Option<String>> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> AppResult<Option<String>> {
        Ok(GitSync::open_or_init(&root)?.get_remote()?)
    })
    .await
    .map_err(join_err)?
}

/// Синхронизация с remote: pull (fast-forward) → push (если не конфликт). Токен берётся из keychain.
/// `MergeRequired` (расхождение истории) → НЕ пушим, сигналим для ручного разрешения (Ф3-3b-3).
#[tauri::command]
pub async fn git_sync(state: State<'_, AppState>) -> AppResult<PullOutcome> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> AppResult<PullOutcome> {
        let account = root.to_string_lossy();
        let token =
            creds::get_token(&account)?.ok_or("нет токена доступа — сохрани его (keychain)")?;
        let git = GitSync::open_or_init(&root)?;
        let pulled = git.pull(&token)?;
        if pulled != PullOutcome::MergeRequired {
            git.push(&token)?;
        }
        Ok(pulled)
    })
    .await
    .map_err(join_err)?
}

/// Превью merge с origin (in-memory `merge_commits`, репозиторий не трогается): up-to-date / clean /
/// конфликты (3-way: base/ours/theirs). Токен из keychain. Для resolver-панели (Ф4-8).
#[tauri::command]
pub async fn git_merge_preview(state: State<'_, AppState>) -> AppResult<MergePreview> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> AppResult<MergePreview> {
        let account = root.to_string_lossy();
        let token =
            creds::get_token(&account)?.ok_or("нет токена доступа — сохрани его (keychain)")?;
        let git = GitSync::open_or_init(&root)?;
        Ok(git.merge_preview(&token)?)
    })
    .await
    .map_err(join_err)?
}

/// Применяет разрешённый merge: `resolutions` (path → итоговое содержимое) + merge-коммит, затем push.
/// `theirs` — oid их коммита из превью. Возвращает oid merge-коммита. Под sync-локом.
#[tauri::command]
pub async fn git_resolve_conflicts(
    state: State<'_, AppState>,
    theirs: String,
    resolutions: Vec<(String, String)>,
) -> AppResult<String> {
    let root = vault_root(&state).await?;
    let _lock = state.git_lock.lock().await;
    tokio::task::spawn_blocking(move || -> AppResult<String> {
        let account = root.to_string_lossy();
        let token =
            creds::get_token(&account)?.ok_or("нет токена доступа — сохрани его (keychain)")?;
        let git = GitSync::open_or_init(&root)?;
        let oid = git.apply_merge(&theirs, &resolutions)?;
        git.push(&token)?;
        Ok(oid)
    })
    .await
    .map_err(join_err)?
}
