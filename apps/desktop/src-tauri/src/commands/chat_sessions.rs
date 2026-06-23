//! Команды сессий чата (решение владельца 2026-06-12): история-дропдаун в AI-панели,
//! «Новая сессия» вместо корзины (ничего не удаляем), экспорт в заметку — явной кнопкой.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::State;

use crate::ai::ChatMessage;
use crate::chat_log::{self, ChatSearchHit, ChatSession, StoredMessage};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::vault;

/// Список сессий (свежие сверху) для дропдауна истории.
#[tauri::command]
pub async fn chat_sessions_list(state: State<'_, AppState>) -> AppResult<Vec<ChatSession>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(chat_log::list_sessions(&reader).await?)
}

/// Полнотекстовый поиск по переписке (#58 session-search): совпавшие сообщения со snippet-
/// подсветкой, заголовком сессии и саммари эпизода (EP). `limit` клампится в ядре (1..=200).
#[tauri::command]
pub async fn chat_search(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
) -> AppResult<Vec<ChatSearchHit>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(chat_log::search_chat(&reader, &query, limit.unwrap_or(50)).await?)
}

/// Сообщения сессии (загрузка в ленту по клику в истории).
#[tauri::command]
pub async fn chat_session_messages(
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<Vec<StoredMessage>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(chat_log::session_messages(&reader, id).await?)
}

/// Пишет завершённый обмен. Первый обмен сессии: создаёт её и асинхронно генерит заголовок
/// мелкой моделью (best-effort: ошибка генерации оставляет плейсхолдер из вопроса).
#[tauri::command]
pub async fn chat_log_exchange(
    state: State<'_, AppState>,
    session_id: Option<i64>,
    question: String,
    answer: String,
    sources_json: Option<String>,
) -> AppResult<i64> {
    let (writer, util, embedder, chat_vectors) = {
        let ctx = state.vault().await?;
        (
            ctx.db.writer().clone(),
            ctx.ai
                .chat_util
                .clone()
                .or_else(|| ctx.ai.chat_fast.clone()),
            ctx.ai.embedder.clone(),
            ctx.chat_vectors.clone(),
        )
    };
    let ex = chat_log::log_exchange(&writer, session_id, &question, &answer, sources_json).await?;
    let sid = ex.session_id;
    let created = ex.created;

    // RAG переписки (N4): индексируем оба сообщения в `chat_vectors` (память «второго мозга»).
    // Best-effort, не блокирует ответ команды. Ключ usearch — id сообщения.
    if let (Some(embedder), Some(vectors)) = (embedder, chat_vectors) {
        let pairs = [
            (ex.user_msg_id, question.clone()),
            (ex.assistant_msg_id, answer.clone()),
        ];
        tokio::spawn(async move {
            for (id, text) in pairs {
                match embedder.embed_documents(&[text.as_str()]).await {
                    Ok(vecs) if !vecs.is_empty() => {
                        if let Err(e) = vectors.upsert(id as u64, &vecs[0]) {
                            tracing::warn!(error = %e, "chat-memory: upsert вектора не удался");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "chat-memory: эмбеддинг сообщения не удался")
                    }
                }
            }
            let _ = vectors.save();
        });
    }

    if created {
        if let Some(util) = util {
            // Заголовок — суммарайз первого вопроса (решение владельца). Не блокируем ответ команды.
            let writer = writer.clone();
            tokio::spawn(async move {
                let messages = [
                    ChatMessage::system(
                        "Сформулируй ОЧЕНЬ короткий заголовок диалога по первому вопросу \
                         пользователя: 3–6 слов, по-русски, без кавычек и точки. Только заголовок.",
                    ),
                    ChatMessage::user(question),
                ];
                let cancel = Arc::new(AtomicBool::new(false));
                let mut out = String::new();
                if util
                    .stream_chat(&messages, &mut |t| out.push_str(&t), &cancel)
                    .await
                    .is_ok()
                {
                    let title = out.trim().trim_matches('"').trim();
                    if !title.is_empty() {
                        let _ = chat_log::set_title(&writer, sid, title).await;
                    }
                }
            });
        }
    }
    Ok(sid)
}

/// Удаляет последний обмен сессии (для регенерации ответа, P6-RGN): убирает прошлую пару из БД и
/// чистит её векторы памяти, чтобы повторный прогон того же вопроса не двоил историю. Best-effort
/// по векторам (могли ещё не проиндексироваться). Без сессии (`None`) — no-op (нечего чистить).
#[tauri::command]
pub async fn chat_delete_last_exchange(
    state: State<'_, AppState>,
    session_id: Option<i64>,
) -> AppResult<()> {
    let Some(sid) = session_id else {
        return Ok(());
    };
    let (writer, chat_vectors) = {
        let ctx = state.vault().await?;
        (ctx.db.writer().clone(), ctx.chat_vectors.clone())
    };
    let removed = chat_log::delete_last_exchange(&writer, sid).await?;
    // Чистим векторы памяти удалённых сообщений (ключ usearch = id сообщения). Best-effort.
    if let Some(vectors) = chat_vectors {
        for id in &removed {
            let _ = vectors.remove(*id as u64);
        }
        if !removed.is_empty() {
            let _ = vectors.save();
        }
    }
    Ok(())
}

/// «Сохранить в заметки»: рендерит сессию в markdown и пишет в `Chats/<дата> <заголовок>.md`
/// (имя санитизируется; коллизия — суффиксом). Возвращает относительный путь заметки.
#[tauri::command]
pub async fn chat_session_to_note(state: State<'_, AppState>, id: i64) -> AppResult<String> {
    let (reader, root) = {
        let ctx = state.vault().await?;
        (ctx.db.reader().clone(), ctx.root.clone())
    };
    let Some((title, md)) = chat_log::session_markdown(&reader, id).await? else {
        return Err(AppError::Msg(format!("сессия не найдена: {id}")));
    };
    // Имя файла: дата + заголовок без запрещённых символов.
    let safe: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let date = chrono_date();
    let mut rel = format!("Chats/{date} {safe}.md");
    let mut n = 2;
    while vault::resolve_vault_path_for_write(&root, Path::new(&rel))
        .map(|p| p.exists())
        .unwrap_or(false)
    {
        rel = format!("Chats/{date} {safe} {n}.md");
        n += 1;
    }
    let abs = vault::resolve_vault_path_for_write(&root, Path::new(&rel))?;
    if let Some(dir) = abs.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    // Атомарно (blocking → spawn_blocking): обрыв не оставляет усечённый экспорт сессии (аудит).
    let bytes = md.into_bytes();
    tokio::task::spawn_blocking(move || vault::atomic_write_io(&abs, &bytes))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))??;
    tracing::info!(path = %rel, "сессия чата сохранена заметкой");
    Ok(rel)
}

/// Локальная дата YYYY-MM-DD без сторонних крейтов (unix-дни от эпохи, civil-алгоритм).
fn chrono_date() -> String {
    let secs = crate::scheduler::now_secs();
    let days = secs / 86_400;
    // Алгоритм Howard Hinnant civil_from_days (для дат 1970+ корректен).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
