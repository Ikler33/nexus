//! Команда краткого резюме заметки для Inspector-rail («Резюме», дизайн Qasr `editor.jsx`). One-shot
//! (не-стрим, как дайджест): текст текущего буфера приходит с фронта (как inline-контекст), резюмируется
//! утилитарной моделью (`ai.fast`, fallback `chat`) через `GuardedClient` (ADR-005). Контент заметки —
//! НЕДОВЕРЕННЫЕ ДАННЫЕ в маркерах (анти-инъекция AC-SEC-7). Без RAG (D2) и без записи.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::State;

use crate::ai::{build_note_summary_messages, injection_marker};
use crate::error::AppResult;
use crate::state::AppState;

/// Потолок символов заметки для резюме — под небольшой контекст утилитарной модели (как inline ~6k, но
/// резюме терпит больше: берём начало, хвост обрезаем). Защита от полотна в IPC/промпте.
const SUMMARY_MAX_CHARS: usize = 12000;

/// Краткое резюме заметки. `text` — полный текст текущего буфера (отражает несохранённые правки, как
/// inline-контекст). `None` — нет chat-провайдера или пустой текст/ответ (UI покажет заглушку, не
/// ошибку); ошибки стрима LLM → `Err` (UI покажет «не удалось» + retry).
#[tauri::command]
pub async fn get_note_summary(
    state: State<'_, AppState>,
    text: String,
) -> AppResult<Option<String>> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    // Утилитарная мелкая модель (как inline/судья); fallback на обычный chat. Нет провайдера → заглушка.
    let chat = {
        let ctx = state.vault().await?;
        ctx.ai.chat_util.clone().or_else(|| ctx.ai.chat.clone())
    };
    let Some(chat) = chat else {
        return Ok(None);
    };

    let capped: String = if text.chars().count() > SUMMARY_MAX_CHARS {
        text.chars().take(SUMMARY_MAX_CHARS).collect()
    } else {
        text
    };
    let messages = build_note_summary_messages(&capped, &injection_marker());

    // Интерактивная LLM-операция (S5): планировщик уступит фоновые LLM-джобы на время резюме.
    let _llm_busy = state.enter_interactive_llm();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut sink = |_t: String| {}; // не-стрим: берём полный текст из результата
    let summary = chat.stream_chat(&messages, &mut sink, &cancel).await?;
    let summary = summary.trim().to_string();
    Ok(if summary.is_empty() {
        None
    } else {
        Some(summary)
    })
}
