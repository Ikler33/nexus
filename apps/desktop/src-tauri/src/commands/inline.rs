//! Команда inline-генерации в редакторе (vision Inline-LLM, AC-IL-*). Стрим результата через `Channel`
//! поверх `ChatProvider` (ADR-005); контекст — текущая заметка (D2), без RAG. Отмена — `inline_cancel`
//! (один активный inline-стрим за раз, AC-IL-8).

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::ai::{build_inline_messages, injection_marker, InlineMode};
use crate::state::AppState;

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
) -> Result<(), String> {
    let mode =
        InlineMode::parse(&mode).ok_or_else(|| format!("неизвестный режим inline: {mode}"))?;

    // Текст для обработки по режиму (D2): выделение для Rewrite/Summarize, текст до курсора для Continue.
    let payload = if mode.needs_selection() {
        let sel = selection.unwrap_or_default();
        if sel.trim().is_empty() {
            return Err("нет выделения для этого действия".into());
        }
        sel
    } else {
        if context.trim().is_empty() {
            return Err("нет текста для продолжения".into());
        }
        context
    };

    // Берём chat-провайдер и отпускаем лок ДО сетевого стрима.
    let chat = {
        let guard = state.vault.read().await;
        let ctx = guard.as_ref().ok_or("vault не открыт")?;
        ctx.chat.clone()
    };
    let Some(chat) = chat else {
        return Err("chat-провайдер не сконфигурирован (.nexus/local.json → ai.chat)".into());
    };

    let messages = build_inline_messages(mode, &payload, &injection_marker());

    // Стрим в канал с отменой: begin_inline отменяет прошлый inline-стрим (один активный, AC-IL-8).
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
pub async fn inline_cancel(state: State<'_, AppState>) -> Result<(), String> {
    state.cancel_active_inline();
    Ok(())
}
