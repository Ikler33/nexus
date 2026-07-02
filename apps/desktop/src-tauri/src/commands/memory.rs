//! Команды памяти агента (MEM, спека `docs/specs/agent-memory.md`): CRUD фактов для панели «Память ИИ»
//! (MEM-4). DB-операции best-effort дополняются индексацией в `memory_vectors` (если есть эмбеддер):
//! провал индексации НЕ валит добавление — факт остаётся в БД (виден в списке/пинах, просто пока не
//! всплывает в семантическом top-k до переиндексации).

use tauri::State;

use crate::error::AppResult;
use crate::memory::consolidate::{
    self, ConsolidationChoice, ConsolidationOutcome, ConsolidationPlan, PlanOp,
};
use crate::memory::{self, MemoryFact, SOURCE_AUTO, SOURCE_EXPLICIT};
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
    let (chat, reader) = {
        let ctx = state.vault().await?;
        (ctx.ai.chat_util.clone(), ctx.db.reader().clone())
    };
    let Some(chat) = chat else {
        return Ok(Vec::new()); // нет «быстрой» модели → без авто-предложений
    };
    let candidates = memory::extract::propose_facts(&chat, &user_text, &assistant_text).await;
    // MEM-3 дедуп предложений: не показывать чип «Запомнить?» на факт, который УЖЕ в памяти (точный
    // повтор) — баг real-test 2026-06-18 (в новом чате повторно предлагал сохранить уже сохранённое имя).
    Ok(memory::filter_known_exact(&reader, candidates).await?)
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

// B6: wire-команда `memory_fact_history` (MEM-7) удалена — «история факта» в UI так и не строилась,
// фронт команду не вызывал (мёртвая API-поверхность). Ядро `memory::fact_history` остаётся, но
// фактически test-only (прод-код его не вызывает); вернуть мост тривиально, когда появится панель.

/// MEM-8 (owner-gated, флаг `aiMemoryConsolidation`): ПОСЧИТАТЬ предложение консолидации для нового
/// факта — НИЧЕГО не пишет. Семантически близкие факты + ОСНОВНАЯ модель (`ctx.ai.chat`, 27B) решают
/// ADD/UPDATE/DELETE→supersede/NOOP. Нет модели/эмбеддера/индекса → fail-closed `Add` (фронт сделает
/// обычный `memory_add`). Применение — отдельной командой [`memory_consolidate_apply`] после выбора.
#[tauri::command]
pub async fn memory_consolidate_plan(
    state: State<'_, AppState>,
    text: String,
    source: Option<String>,
) -> AppResult<ConsolidationPlan> {
    let source = match source.as_deref() {
        Some(SOURCE_AUTO) => SOURCE_AUTO,
        _ => SOURCE_EXPLICIT,
    };
    let (reader, embedder, vectors, chat) = {
        let ctx = state.vault().await?;
        (
            ctx.db.reader().clone(),
            ctx.ai.embedder.clone(),
            ctx.memory_vectors.clone(),
            ctx.ai.chat.clone(),
        )
    };
    // Нет основной модели / эмбеддера / индекса → консолидация невозможна, fail-closed ADD.
    let (Some(embedder), Some(vectors), Some(chat)) = (embedder, vectors, chat) else {
        return Ok(ConsolidationPlan {
            candidate: text.trim().to_string(),
            source: source.to_string(),
            op: PlanOp::Add,
        });
    };
    Ok(consolidate::plan(&reader, &vectors, embedder.as_ref(), &chat, &text, source).await?)
}

/// MEM-8: ПРИМЕНИТЬ выбор пользователя к предложению — в одной транзакции (с optimistic-чеком), затем
/// индексация по результату (best-effort, как `memory_add`). Возвращает что РЕАЛЬНО произошло
/// ([`ConsolidationOutcome`]) — фронт покажет toast, будущий откат таргетит `opGroup`.
#[tauri::command]
pub async fn memory_consolidate_apply(
    state: State<'_, AppState>,
    plan: ConsolidationPlan,
    choice: ConsolidationChoice,
) -> AppResult<ConsolidationOutcome> {
    let (writer, embedder, vectors) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.ai.embedder.clone(),
            ctx.memory_vectors.clone(),
        )
    };
    let candidate = plan.candidate.clone();
    let outcome = consolidate::apply(&writer, plan, choice).await?;
    if let (Some(emb), Some(vec)) = (embedder, vectors) {
        match &outcome {
            ConsolidationOutcome::Add { id, inserted } => {
                if *inserted {
                    let _ = memory::index_fact(&vec, emb.as_ref(), *id, candidate.trim()).await;
                }
            }
            ConsolidationOutcome::Update { id, new_text, .. } => {
                let _ = memory::index_fact(&vec, emb.as_ref(), *id, new_text).await;
            }
            ConsolidationOutcome::Supersede {
                id,
                superseded_id,
                new_text,
                inserted,
                ..
            } => {
                // ПОРЯДОК ВАЖЕН: сперва индексируем НОВЫЙ, потом убираем старый из ANN. Индексация
                // best-effort (вне writer-tx) — при сбое между шагами лучше over-recall (оба в индексе),
                // чем дыра (ни одного до реиндекса). Не переставлять (находка ревью).
                if *inserted {
                    let _ = memory::index_fact(&vec, emb.as_ref(), *id, new_text).await;
                }
                // Супридённый факт убираем из ANN-индекса — иначе всплыл бы в ретривале (хотя
                // `facts_by_ids` его и отфильтрует по `superseded_by`, держим индекс в согласии с БД).
                let _ = memory::unindex_fact(&vec, *superseded_id);
            }
            ConsolidationOutcome::Noop => {}
        }
    }
    Ok(outcome)
}

/// MEM-8c-b: ОТКАТ группы консолидации по `opGroup` (§4.6) — undo авто-режима / чипа. Реверсит
/// `update`/`supersede`/`add` группы (optimistic-безопасно) + переиндексирует. Возвращает `true`, если
/// что-то реально откатилось (фронт обновит toast/панель).
#[tauri::command]
pub async fn memory_consolidate_undo(state: State<'_, AppState>, op_group: i64) -> AppResult<bool> {
    let (writer, embedder, vectors) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.ai.embedder.clone(),
            ctx.memory_vectors.clone(),
        )
    };
    let plan = consolidate::undo(&writer, op_group).await?;
    if let (Some(emb), Some(vec)) = (embedder, vectors) {
        for (id, text) in &plan.reindex {
            let _ = memory::index_fact(&vec, emb.as_ref(), *id, text).await;
        }
        for id in &plan.unindex {
            let _ = memory::unindex_fact(&vec, *id);
        }
    }
    Ok(plan.reverted())
}
