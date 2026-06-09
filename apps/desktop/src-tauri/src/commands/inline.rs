//! Команда inline-генерации в редакторе (vision Inline-LLM, AC-IL-*). Стрим результата через `Channel`
//! поверх `ChatProvider` (ADR-005); контекст — текущая заметка (D2), без RAG. Отмена — `inline_cancel`
//! (один активный inline-стрим за раз, AC-IL-8).

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::ai::{build_inline_messages, injection_marker, InlineMode};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Макс. символов inline-контекста — под небольшой контекст утилитарной модели (Qwen3-4B, 4k токенов):
/// ~6000 симв кириллицы ≈ ~2.5–3k токенов, остаётся место под систему/ответ. На gemma-fallback безвредно.
const INLINE_MAX_CHARS: usize = 6000;

/// Обрезает текст до [`INLINE_MAX_CHARS`]: `keep_start=true` — оставляем начало (выделение для
/// rewrite/summarize), иначе — хвост (continue: важен текст у курсора).
fn cap_chars(s: String, keep_start: bool) -> String {
    let n = s.chars().count();
    if n <= INLINE_MAX_CHARS {
        return s;
    }
    if keep_start {
        s.chars().take(INLINE_MAX_CHARS).collect()
    } else {
        s.chars().skip(n - INLINE_MAX_CHARS).collect()
    }
}

/// Событие inline-стрима для фронта (дискриминированное по `type`, camelCase). Без `Sources`: inline
/// не делает RAG-ретрив (D2 — контекст = текущая заметка).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum InlineStreamEvent {
    /// Очередная текстовая дельта (ghost-text).
    Token { text: String },
    /// Поток завершён штатно; `full` — полный сгенерированный текст.
    Done { full: String },
    /// Ошибка на этапе LLM-стрима.
    Error { message: String },
}

/// Inline-генерация со стримингом в редактор (AC-IL-1). `mode` — `continue`/`rewrite`/`summarize`;
/// `context` — текст заметки до курсора (для `continue`); `selection` — выделенный фрагмент (для
/// `rewrite`/`summarize`, D4). Результат стримится в `channel`; отмена — `inline_cancel` (AC-IL-6/8).
/// Ошибки настройки (нет vault/chat, пустой ввод, неизвестный режим) → `Err` (фронт покажет тихую
/// inline-нотификацию у курсора, AC-IL-7); ошибки стрима → событие `Error`.
#[tauri::command]
pub async fn inline_complete(
    state: State<'_, AppState>,
    channel: Channel<InlineStreamEvent>,
    mode: String,
    context: String,
    selection: Option<String>,
) -> AppResult<()> {
    let mode = InlineMode::parse(&mode)
        .ok_or_else(|| AppError::Msg(format!("неизвестный режим inline: {mode}")))?;

    // Текст для обработки по режиму (D2): выделение для Rewrite/Summarize, текст до курсора для Continue.
    // Капим под небольшой контекст утилитарной модели (4k): для continue важен хвост (у курсора),
    // для выделения — начало.
    let payload = if mode.needs_selection() {
        let sel = selection.unwrap_or_default();
        if sel.trim().is_empty() {
            return Err("нет выделения для этого действия".into());
        }
        cap_chars(sel, true)
    } else {
        if context.trim().is_empty() {
            return Err("нет текста для продолжения".into());
        }
        cap_chars(context, false)
    };

    // Утилитарная мелкая модель (`ai.fast`, напр. Qwen3-4B :8084) — для inline низкая латентность.
    // `chat_util` уже с fallback на gemma-fast (см. open_vault); тут ещё fallback на обычный chat.
    let chat = {
        let ctx = state.vault().await?;
        ctx.chat_util.clone().or_else(|| ctx.chat.clone())
    };
    let Some(chat) = chat else {
        return Err("chat-провайдер не сконфигурирован (.nexus/local.json → ai.chat)".into());
    };

    let messages = build_inline_messages(mode, &payload, &injection_marker());

    // Стрим в канал с отменой: begin_inline отменяет прошлый inline-стрим (один активный, AC-IL-8).
    // Помечаем интерактивную LLM-операцию (S5) — планировщик уступит фоновые LLM-джобы, пока идёт inline.
    let _llm_busy = state.enter_interactive_llm();
    let cancel = state.begin_inline();
    let result = {
        let mut on_token = |t: String| {
            let _ = channel.send(InlineStreamEvent::Token { text: t });
        };
        chat.stream_chat(&messages, &mut on_token, &cancel).await
    };
    match result {
        Ok(full) => {
            let _ = channel.send(InlineStreamEvent::Done { full });
        }
        Err(e) => {
            let _ = channel.send(InlineStreamEvent::Error {
                message: e.to_string(),
            });
        }
    }
    Ok(())
}

/// Отменяет активный inline-стрим (если есть). Идемпотентно (AC-IL-6).
#[tauri::command]
pub async fn inline_cancel(state: State<'_, AppState>) -> AppResult<()> {
    state.cancel_active_inline();
    Ok(())
}
