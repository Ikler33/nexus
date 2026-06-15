//! Команды памяти агента (MEM, спека `docs/specs/agent-memory.md`): CRUD фактов для панели «Память ИИ»
//! (MEM-4). DB-операции best-effort дополняются индексацией в `memory_vectors` (если есть эмбеддер):
//! провал индексации НЕ валит добавление — факт остаётся в БД (виден в списке/пинах, просто пока не
//! всплывает в семантическом top-k до переиндексации).

use tauri::State;

use crate::error::AppResult;
use crate::memory::{self, MemoryFact, SOURCE_EXPLICIT};
use crate::state::AppState;

/// AC-MEM-2: список фактов (пины сверху, затем по дате).
#[tauri::command]
pub async fn memory_list(state: State<'_, AppState>) -> AppResult<Vec<MemoryFact>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(memory::list(&reader).await?)
}

/// AC-MEM-1: явно добавить факт (+ проиндексировать, если есть эмбеддер). Возвращает id (или `None` —
/// пустой текст). `source='explicit'` (D1).
#[tauri::command]
pub async fn memory_add(state: State<'_, AppState>, text: String) -> AppResult<Option<i64>> {
    let (writer, embedder, vectors) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.ai.embedder.clone(),
            ctx.memory_vectors.clone(),
        )
    };
    let id = memory::add(&writer, &text, SOURCE_EXPLICIT).await?;
    if let (Some(id), Some(emb), Some(vec)) = (id, embedder, vectors) {
        let _ = memory::index_fact(&vec, emb.as_ref(), id, text.trim()).await;
    }
    Ok(id)
}

/// AC-MEM-3: пин/анпин факта.
#[tauri::command]
pub async fn memory_set_pinned(state: State<'_, AppState>, id: i64, pinned: bool) -> AppResult<()> {
    let writer = state.vault().await?.db.writer().clone();
    memory::set_pinned(&writer, id, pinned).await?;
    Ok(())
}

/// AC-MEM-3: правка текста факта (+ ре-эмбеддинг — upsert перезаписывает вектор по тому же id).
#[tauri::command]
pub async fn memory_edit(state: State<'_, AppState>, id: i64, text: String) -> AppResult<()> {
    let (writer, embedder, vectors) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.ai.embedder.clone(),
            ctx.memory_vectors.clone(),
        )
    };
    memory::edit(&writer, id, &text).await?;
    // Ре-эмбеддим ТОЛЬКО при непустом тексте: `memory::edit` на пустом — no-op (текст в БД не меняется),
    // поэтому индексировать embedding пустой строки нельзя — это перезаписало бы корректный вектор факта
    // и рассинхронило бы индекс с БД.
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        if let (Some(emb), Some(vec)) = (embedder, vectors) {
            let _ = memory::index_fact(&vec, emb.as_ref(), id, trimmed).await;
        }
    }
    Ok(())
}

/// AC-MEM-3: удалить факт (+ убрать из индекса).
#[tauri::command]
pub async fn memory_delete(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let (writer, vectors) = {
        let ctx = state.vault().await?;
        (ctx.db.writer().clone(), ctx.memory_vectors.clone())
    };
    memory::delete(&writer, id).await?;
    if let Some(vec) = vectors {
        let _ = memory::unindex_fact(&vec, id);
    }
    Ok(())
}
