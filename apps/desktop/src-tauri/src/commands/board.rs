//! Команды канбан-доски: BOARD-2 (выборка задач) + BOARD-3 (персист конфига колонок/порядка/scope).

use std::collections::HashSet;

use serde::Serialize;
use tauri::State;

use crate::board::config::{self, BoardConfig, BoardSummary};
use crate::board::{self, StaleTask, TaskCard, DEFAULT_STATUS_KEY};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Порог «застряло» по умолчанию (дней) — AI-2a; фронт может переопределить.
const DEFAULT_STALE_DAYS: i64 = 14;

/// Все заметки-задачи (есть frontmatter-ключ `status_key`, по умолч. `status`) с полями для доски.
/// Без открытого vault — ошибка. Чистый SQL-read (офлайн, без LLM/сети). Колонкование — на фронте.
#[tauri::command]
pub async fn list_board(
    state: State<'_, AppState>,
    status_key: Option<String>,
) -> AppResult<Vec<TaskCard>> {
    let reader = state.vault().await?.db.reader().clone();
    let key = status_key
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_STATUS_KEY.to_string());
    Ok(board::list_board(&reader, key).await?)
}

/// AI-2a (A2): «застрявшие» задачи — заметки-задачи, не правленные ≥ `threshold_days` (умолч. 14) дней по
/// `edit_events` (фолбэк mtime). Детерминированный SQL-read (без LLM/сети). `now` — серверное время.
/// Done-like-статусы НЕ отсеиваются здесь (фронт фильтрует по конфигу доски).
#[tauri::command]
pub async fn stale_tasks(
    state: State<'_, AppState>,
    status_key: Option<String>,
    threshold_days: Option<i64>,
) -> AppResult<Vec<StaleTask>> {
    let reader = state.vault().await?.db.reader().clone();
    let key = status_key
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_STATUS_KEY.to_string());
    let days = threshold_days.unwrap_or(DEFAULT_STALE_DAYS).max(1);
    Ok(board::stale_tasks(&reader, key, days, crate::scheduler::now_secs()).await?)
}

/// Доска целиком (BOARD-3): персист-конфиг (колонки/порядок/scope) + карточки в его scope/statusKey.
/// `corrupt` — JSON конфига был, но битый → фронт-тост (используется дефолт, файл НЕ перезаписан вслепую).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardData {
    pub config: BoardConfig,
    pub cards: Vec<TaskCard>,
    pub corrupt: bool,
}

/// Загружает доску `slug` (умолч. `personal`): конфиг + карточки. order самозалечивается — GC удалённых/
/// вне-scope путей; при изменении и ВАЛИДНОМ конфиге best-effort persist (битый не перезаписываем).
#[tauri::command]
pub async fn get_board(state: State<'_, AppState>, slug: Option<String>) -> AppResult<BoardData> {
    let ctx = state.vault().await?;
    let root = ctx.root.clone();
    let reader = ctx.db.reader().clone();
    let id = slug
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| config::BOARD_ID_DEFAULT.to_string());

    let root_l = root.clone();
    let id_l = id.clone();
    let loaded = tokio::task::spawn_blocking(move || config::load(&root_l, &id_l))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?;
    let mut cfg = loaded.config;

    let all = board::list_board(&reader, cfg.status_key.clone()).await?;
    let cards: Vec<TaskCard> = all
        .into_iter()
        .filter(|c| config::matches_scope(c, &cfg.scope))
        .collect();

    // GC порядка — ТОЛЬКО для отображения (in-memory): убираем из возвращаемого order пути вне текущих
    // карточек. НЕ персистим на чтении! (adversarial-ревью F1, класс MEM-5): при холодном/отстающем
    // индексе `list_board` отдаёт меньше карточек, чем есть на диске → персист GC СТЁР БЫ ручной порядок
    // живых задач. Чистим персист ТОЧЕЧНО на реальном удалении (delete-хук в `delete_path`) и реордере
    // (`save_board`), а не вслепую по «отсутствию в выборке». Стейл-записи в файле безвредны (applyOrder
    // их игнорирует).
    let existing: HashSet<&str> = cards.iter().map(|c| c.path.as_str()).collect();
    config::gc_order(&mut cfg, &existing);

    Ok(BoardData {
        config: cfg,
        cards,
        corrupt: loaded.corrupt,
    })
}

/// Персистит конфиг доски (BOARD-3): переименование/реордер колонок, ручной порядок (DnD — BOARD-5).
#[tauri::command]
pub async fn save_board(state: State<'_, AppState>, config: BoardConfig) -> AppResult<()> {
    let root = state.vault().await?.root.clone();
    tokio::task::spawn_blocking(move || config::save(&root, &config))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?
        .map_err(AppError::Io)?;
    Ok(())
}

/// Список досок (`.nexus/boards/*.json`); пусто → синтетический дефолт (всегда ≥1 доска для UI).
#[tauri::command]
pub async fn list_boards(state: State<'_, AppState>) -> AppResult<Vec<BoardSummary>> {
    let root = state.vault().await?.root.clone();
    tokio::task::spawn_blocking(move || config::list_boards(&root))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))
}
