//! Команды графа/беклинков (ADR-004).

use tauri::State;

use crate::graph::{self, BacklinkEntry, FullGraph, GraphData};
use crate::state::AppState;

/// Беклинки файла (источник истины — SQLite, запрос по idx_links_target).
#[tauri::command]
pub async fn get_backlinks(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<BacklinkEntry>, String> {
    let reader = reader(&state).await?;
    graph::get_backlinks(&reader, path)
        .await
        .map_err(|e| e.to_string())
}

/// Локальный N-hop граф вокруг файла (ADR-004).
#[tauri::command]
pub async fn get_local_graph(
    state: State<'_, AppState>,
    center: String,
    hops: u32,
) -> Result<GraphData, String> {
    let reader = reader(&state).await?;
    graph::get_local_graph(&reader, center, hops)
        .await
        .map_err(|e| e.to_string())
}

/// Единый граф всего vault (AC-DOD-Ф3): топ-`limit` файлов по связности + рёбра.
#[tauri::command]
pub async fn get_full_graph(
    state: State<'_, AppState>,
    limit: usize,
) -> Result<FullGraph, String> {
    let reader = reader(&state).await?;
    graph::get_full_graph(&reader, limit)
        .await
        .map_err(|e| e.to_string())
}

async fn reader(state: &State<'_, AppState>) -> Result<crate::db::ReadPool, String> {
    let guard = state.vault.read().await;
    Ok(guard.as_ref().ok_or("vault не открыт")?.db.reader().clone())
}
