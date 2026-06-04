//! Команда RAG-чата (Ф1-7): retrieve (гибрид) → промпт → стриминг ответа через `Channel` (§4.1).
//!
//! Поток: сперва `Sources` (найденные чанки), затем поток `Token`, в конце `Done` (или `Error`).
//! Отмена — `chat_cancel` (взводит флаг активного стрима; см. [`AppState::begin_chat`]).

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::ai::{build_chat_messages, build_rag_messages};
use crate::search::{self, SearchHit, SearchOptions};
use crate::state::AppState;

/// Событие чат-стрима для фронта (дискриминированное по `type`, camelCase).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatStreamEvent {
    /// Источники (найденные RAG-чанки) — приходит первым, до токенов.
    Sources { sources: Vec<SearchHit> },
    /// Очередная текстовая дельта ответа.
    Token { text: String },
    /// Поток завершён штатно; `full` — полный текст ответа (для записи в историю).
    Done { full: String },
    /// Ошибка на любом этапе (retrieve/LLM); стрим завершается.
    Error { message: String },
}

/// Кол-во RAG-чанков в контексте по умолчанию (калибруется eval-харнессом, Ф1-10).
const DEFAULT_K: usize = 8;

/// Чат со стримингом. `grounded` (по умолчанию `true`) — режим «по vault»: RAG-ретрив → источники →
/// промпт с контекстом. `grounded=false` — **общий чат** (V4.4): БЕЗ ретрива, ответ напрямую от
/// модели (источники пустые). Ответ стримится в `channel`.
#[tauri::command]
pub async fn chat_rag(
    state: State<'_, AppState>,
    channel: Channel<ChatStreamEvent>,
    question: String,
    k: Option<usize>,
    center: Option<String>,
    grounded: Option<bool>,
) -> Result<(), String> {
    let grounded = grounded.unwrap_or(true);
    // Снимаем нужное из контекста и отпускаем лок ДО сетевых вызовов (эмбеддинг + LLM-стрим).
    let (reader, vectors, embedder, chat) = {
        let guard = state.vault.read().await;
        let ctx = guard.as_ref().ok_or("vault не открыт")?;
        (
            ctx.db.reader().clone(),
            ctx.vectors.clone(),
            ctx.embedder.clone(),
            ctx.chat.clone(),
        )
    };
    let Some(chat) = chat else {
        return Err("chat-провайдер не сконфигурирован (.nexus/local.json → ai.chat)".into());
    };

    // Сборка сообщений: vault-режим (RAG-ретрив + источники) ИЛИ общий чат (без грунтинга, V4.4).
    let messages = if grounded {
        let k = k.unwrap_or(DEFAULT_K).clamp(1, 20);
        // 1) Retrieve: гибридный поиск (с граф-рангом от открытого файла, если задан) → источники.
        let opts = SearchOptions {
            limit: k,
            filter: None,
            center,
        };
        let hits = match search::hybrid_search(
            &reader,
            vectors.as_deref(),
            embedder.as_deref(),
            question.clone(),
            opts,
        )
        .await
        {
            Ok(h) => h,
            Err(e) => {
                let _ = channel.send(ChatStreamEvent::Error {
                    message: e.to_string(),
                });
                return Ok(());
            }
        };
        let _ = channel.send(ChatStreamEvent::Sources {
            sources: hits.clone(),
        });

        // 2) Контекст из полного содержимого чанков (в порядке релевантности).
        let ids: Vec<i64> = hits.iter().map(|h| h.chunk_id).collect();
        let texts = search::fetch_chunk_contexts(&reader, &ids)
            .await
            .map_err(|e| e.to_string())?;
        let contexts: Vec<(String, String)> = hits
            .iter()
            .filter_map(|h| texts.get(&h.chunk_id).cloned())
            .collect();
        build_rag_messages(&question, &contexts)
    } else {
        // V4.4: общий чат — ретрив НЕ выполняется. Пустые источники, чтобы UI очистил прежние.
        let _ = channel.send(ChatStreamEvent::Sources {
            sources: Vec::new(),
        });
        build_chat_messages(&question)
    };

    // 3) Стриминг ответа в канал (с поддержкой отмены).
    let cancel = state.begin_chat();
    let result = {
        let mut on_token = |t: String| {
            let _ = channel.send(ChatStreamEvent::Token { text: t });
        };
        chat.stream_chat(&messages, &mut on_token, &cancel).await
    };
    match result {
        Ok(full) => {
            let _ = channel.send(ChatStreamEvent::Done { full });
        }
        Err(e) => {
            let _ = channel.send(ChatStreamEvent::Error {
                message: e.to_string(),
            });
        }
    }
    Ok(())
}

/// Отменяет активный чат-стрим (если есть). Идемпотентно.
#[tauri::command]
pub async fn chat_cancel(state: State<'_, AppState>) -> Result<(), String> {
    state.cancel_active_chat();
    Ok(())
}
