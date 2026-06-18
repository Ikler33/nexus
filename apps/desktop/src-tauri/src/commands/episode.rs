//! Команды эпизодической памяти (EP-3, спека `docs/specs/agent-episodic-memory.md` §7/§9): панель
//! эпизодов + обратимость. `dismiss`/`restore` — мягкое скрытие (обратимо). `purge` — жёсткое удаление
//! (DELETE строки + вектор из `episode_vectors`): реальный путь стереть саммари, т.к. CASCADE мёртв
//! (команды удаления сессии нет). Тоггл `episodic.enabled` persisted; включение enqueue'ит kick-джобу
//! (контракт MAJOR-2 из adversarial-ревью EP-1: иначе фича «мертва до перезапуска vault»).

use tauri::State;

use crate::episode::{self, EpisodeRow, KIND_EPISODE_ROLLUP};
use crate::error::AppResult;
use crate::state::AppState;

/// EP-3: все эпизоды для панели (обратная хронология, со скрытыми — панель даёт «восстановить»).
#[tauri::command]
pub async fn episode_list(state: State<'_, AppState>) -> AppResult<Vec<EpisodeRow>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(episode::list(&reader).await?)
}

/// EP-3: скрыть эпизод (обратимо — убирает из ретривала, строка/вектор живы).
#[tauri::command]
pub async fn episode_dismiss(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let writer = state.vault().await?.db.writer().clone();
    episode::set_dismissed(&writer, id, true).await?;
    Ok(())
}

/// EP-3: восстановить скрытый эпизод.
#[tauri::command]
pub async fn episode_restore(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let writer = state.vault().await?.db.writer().clone();
    episode::set_dismissed(&writer, id, false).await?;
    Ok(())
}

/// EP-3: удалить эпизод НАВСЕГДА — DELETE строки + удаление вектора из `episode_vectors` (зеркало
/// `memory_delete`). Необратимо. Первоисточник (сессия/сообщения) не трогается.
#[tauri::command]
pub async fn episode_purge(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let (writer, vectors) = {
        let ctx = state.vault().await?;
        (ctx.db.writer().clone(), ctx.episode_vectors.clone())
    };
    episode::purge(&writer, id).await?;
    if let Some(vec) = vectors {
        let _ = vec.remove(id as u64);
        let _ = vec.save();
    }
    Ok(())
}

/// EP-3: текущее состояние тоггла эпизодической памяти (persisted).
#[tauri::command]
pub async fn episode_get_enabled(state: State<'_, AppState>) -> AppResult<bool> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(episode::is_enabled(&reader).await)
}

/// EP-3: переключить эпизодическую память. Persist `episodic.enabled` + при ВКЛЮЧЕНИИ — enqueue
/// `episode_rollup` kick (контракт MAJOR-2: seed гейтится `is_enabled`, recurring бутстрапится только из
/// успешного прогона → без kick включение в работающем приложении не запустит генерацию до перезапуска
/// vault). Handler сам рано выйдет NOOP, если состояние рассинхронится.
#[tauri::command]
pub async fn episode_set_enabled(state: State<'_, AppState>, on: bool) -> AppResult<()> {
    let writer = state.vault().await?.db.writer().clone();
    episode::set_enabled(&writer, on).await?;
    if on {
        let _ = crate::scheduler::enqueue(&writer, KIND_EPISODE_ROLLUP, "", 0, 2).await;
    }
    Ok(())
}
