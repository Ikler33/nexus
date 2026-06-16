//! Команда предложений связей (Ф1-9, режим 1 max-sim). Считается из готовых векторов usearch.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;
use crate::suggest::{self, LinkSuggestion};
use crate::tagger::{self, TagSuggestion};

/// Максимум тегов словаря в промпте авто-тега (бюджет токенов): `list_tags` возвращает по убыванию частоты,
/// берём топ — самые «обжитые» теги полезнее для классификации; редкие хвосты в промпт не тащим.
const AUTOTAG_VOCAB_CAP: usize = 120;

/// Кандидаты на связь для файла `path` (семантически близкие незалинкованные заметки).
/// `limit` по умолчанию 5, потолок 20. Без RAG-индекса (нет векторов) — пусто.
#[tauri::command]
pub async fn get_link_suggestions(
    state: State<'_, AppState>,
    path: String,
    limit: Option<usize>,
) -> AppResult<Vec<LinkSuggestion>> {
    let (reader, vectors) = {
        let ctx = state.vault().await?;
        (ctx.db.reader().clone(), ctx.vectors.clone())
    };
    let Some(vectors) = vectors else {
        return Ok(Vec::new());
    };
    let limit = limit.unwrap_or(5).min(20);
    Ok(suggest::get_link_suggestions(&reader, vectors.as_ref(), path, limit).await?)
}

/// «Похожие заметки» (#35, дискавери): семантически близкие заметки ВКЛЮЧАЯ уже связанные. Порог —
/// на стороне UI (настройка), бэкенд отдаёт топ-`limit` по max-sim. `limit` по умолчанию 12, потолок 20.
#[tauri::command]
pub async fn get_related_notes(
    state: State<'_, AppState>,
    path: String,
    limit: Option<usize>,
) -> AppResult<Vec<LinkSuggestion>> {
    let (reader, vectors) = {
        let ctx = state.vault().await?;
        (ctx.db.reader().clone(), ctx.vectors.clone())
    };
    let Some(vectors) = vectors else {
        return Ok(Vec::new());
    };
    let limit = limit.unwrap_or(12).min(20);
    Ok(suggest::get_related_notes(&reader, vectors.as_ref(), path, limit).await?)
}

/// AIP-10: короткое LLM-объяснение, ЧЕМ связаны две заметки (для карточек «Связи»/«Похожие» вместо
/// сырого сниппета). Лениво по видимой карточке + кэш `relation_reasons`. Нет утилитарной модели
/// (`chat_util`) ИЛИ пустой контент ИЛИ ошибка LLM → ПУСТАЯ строка (НЕ ошибка) — фронт показывает
/// сниппет как фолбэк, без toast-спама на каждую карточку.
#[tauri::command]
pub async fn explain_relation(
    state: State<'_, AppState>,
    path_a: String,
    path_b: String,
) -> AppResult<String> {
    let (reader, writer, chat) = {
        let ctx = state.vault().await?;
        (
            ctx.db.reader().clone(),
            ctx.db.writer().clone(),
            ctx.ai.chat_util.clone(),
        )
    };
    let Some(chat) = chat else {
        return Ok(String::new()); // нет утилитарной модели → фронт покажет сниппет
    };
    Ok(crate::relation_reasons::explain_relation(&reader, &chat, &writer, path_a, path_b).await?)
}

/// AIP-SQ: до 3 коротких стартовых вопросов по активной заметке `center` для ПУСТОГО чата (снимает
/// «проблему чистого листа»). Нет утилитарной модели / нет заметки / пустой контент / ошибка LLM →
/// ПУСТОЙ список (НЕ ошибка) — фронт показывает статические подсказки, без toast. Кэш — на фронте.
#[tauri::command]
pub async fn get_starting_questions(
    state: State<'_, AppState>,
    center: Option<String>,
) -> AppResult<Vec<String>> {
    let (reader, chat) = {
        let ctx = state.vault().await?;
        (ctx.db.reader().clone(), ctx.ai.chat_util.clone())
    };
    Ok(
        crate::starting_questions::starting_questions(&reader, chat.as_ref(), center.as_deref())
            .await?,
    )
}

/// AI-2c (A4): closed-vocab авто-тег. По содержимому заметки `path` `chat_util` предлагает теги ТОЛЬКО из
/// словаря vault (топ-частотных, кап `AUTOTAG_VOCAB_CAP`). Нет утилитарной модели / нет контента / нет
/// тегов в vault → пусто (НЕ ошибка) — фронт покажет «нет предложений». Никогда НЕ пишет (применение — по
/// явному клику на фронте через `write_file`). Закрытость словаря — на выходе `tagger::parse_and_filter`.
#[tauri::command]
pub async fn suggest_tags(state: State<'_, AppState>, path: String) -> AppResult<TagSuggestion> {
    let (reader, chat) = {
        let ctx = state.vault().await?;
        (ctx.db.reader().clone(), ctx.ai.chat_util.clone())
    };
    let Some(chat) = chat else {
        return Ok(TagSuggestion::default()); // нет утилитарной модели → нет предложений
    };
    let snippet = crate::contradictions::note_snippet(&reader, &path).await?;
    let vocab: Vec<String> = crate::tags::list_tags(&reader)
        .await?
        .into_iter()
        .take(AUTOTAG_VOCAB_CAP)
        .map(|t| t.name)
        .collect();
    let cancel = Arc::new(AtomicBool::new(false));
    Ok(tagger::classify_tags(&chat, &vocab, &snippet, &cancel).await)
}
