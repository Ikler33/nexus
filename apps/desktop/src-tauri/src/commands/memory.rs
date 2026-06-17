//! Команды памяти агента (MEM, спека `docs/specs/agent-memory.md`): CRUD фактов для панели «Память ИИ»
//! (MEM-4). DB-операции best-effort дополняются индексацией в `memory_vectors` (если есть эмбеддер):
//! провал индексации НЕ валит добавление — факт остаётся в БД (виден в списке/пинах, просто пока не
//! всплывает в семантическом top-k до переиндексации).

use tauri::State;

use crate::error::AppResult;
use crate::memory::{self, FactEvent, MemoryFact, SOURCE_AUTO, SOURCE_EXPLICIT};
use crate::state::AppState;

/// AC-MEM-2: список фактов (пины сверху, затем по дате).
#[tauri::command]
pub async fn memory_list(state: State<'_, AppState>) -> AppResult<Vec<MemoryFact>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(memory::list(&reader).await?)
}

/// Результат `memory_add` для фронта: id факта + `inserted` (новая строка vs дубль). MEM-5: «Отменить»
/// удаляет факт ТОЛЬКО при `inserted=true` — иначе undo стёр бы существующий факт пользователя.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAddResult {
    pub id: i64,
    pub inserted: bool,
}

/// AC-MEM-1/6: добавить факт (+ проиндексировать НОВЫЙ, если есть эмбеддер). Возвращает `{id, inserted}`
/// (или `None` — пустой текст). `source` (D1): `'explicit'` (явная команда/кнопка, дефолт) либо `'auto'`
/// (подтверждённое авто-предложение). Любое иное значение трактуется как `'explicit'` (fail-safe).
#[tauri::command]
pub async fn memory_add(
    state: State<'_, AppState>,
    text: String,
    source: Option<String>,
) -> AppResult<Option<MemoryAddResult>> {
    let source = match source.as_deref() {
        Some(SOURCE_AUTO) => SOURCE_AUTO,
        _ => SOURCE_EXPLICIT,
    };
    let (writer, embedder, vectors) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.ai.embedder.clone(),
            ctx.memory_vectors.clone(),
        )
    };
    let added = memory::add(&writer, &text, source).await?;
    // Индексируем только НОВЫЙ факт — дубль уже в индексе (и его вектор не трогаем).
    if let (Some((id, true)), Some(emb), Some(vec)) = (added, embedder, vectors) {
        let _ = memory::index_fact(&vec, emb.as_ref(), id, text.trim()).await;
    }
    Ok(added.map(|(id, inserted)| MemoryAddResult { id, inserted }))
}

/// AC-MEM-6 (D1, MEM-9): авто-ПРЕДЛОЖЕНИЕ фактов по последнему обмену — «быстрая» модель извлекает
/// 0..N кандидатов. НИЧЕГО НЕ ПИШЕТ: фронт показывает чипы «Запомнить? ✓/✗», запись — лишь после ✓
/// через `memory_add`. Нет утилитарной модели / нечего предлагать / ошибка LLM → пустой список.
#[tauri::command]
pub async fn memory_propose(
    state: State<'_, AppState>,
    user_text: String,
    assistant_text: String,
) -> AppResult<Vec<String>> {
    let chat = state.vault().await?.ai.chat_util.clone();
    let Some(chat) = chat else {
        return Ok(Vec::new()); // нет «быстрой» модели → без авто-предложений
    };
    Ok(memory::extract::propose_facts(&chat, &user_text, &assistant_text).await)
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

/// MEM-7: история событий факта (правки/удаление/замещение) — для «истории факта» в панели.
#[tauri::command]
pub async fn memory_fact_history(state: State<'_, AppState>, id: i64) -> AppResult<Vec<FactEvent>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(memory::fact_history(&reader, id).await?)
}
