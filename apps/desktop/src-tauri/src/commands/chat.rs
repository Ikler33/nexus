//! Команда RAG-чата (Ф1-7): retrieve (гибрид) → промпт → стриминг ответа через `Channel` (§4.1).
//!
//! Поток: сперва `Sources` (найденные чанки), затем поток `Token`, в конце `Done` (или `Error`).
//! Отмена — `chat_cancel` (взводит флаг активного стрима; см. [`AppState::begin_chat`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::ai::{
    build_chat_messages, build_rag_messages, build_web_answer_messages, injection_marker,
    ChatMessage, ChatProvider,
};
use crate::error::AppResult;
use crate::search::{self, SearchHit, SearchOptions};
use crate::state::AppState;

/// Событие чат-стрима для фронта (дискриминированное по `type`, camelCase).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatStreamEvent {
    /// Источники (найденные RAG-чанки) — приходит первым, до токенов.
    Sources { sources: Vec<SearchHit> },
    /// Web-источники (W-2): результаты поиска SearXNG (title/url/snippet) — цитаты web-ответа.
    WebSources {
        sources: Vec<crate::websearch::SearchResult>,
    },
    /// Очередная текстовая дельта ответа.
    Token { text: String },
    /// Сырая дельта «размышления» reasoning-модели (R1) — для спойлера «развернуть».
    Reasoning { text: String },
    /// Короткая ЖИВАЯ сводка размышления (мелкая модель суммаризует CoT) — «💭 …», обновляется по ходу.
    ReasoningSummary { text: String },
    /// Поток завершён штатно; `full` — полный текст ответа (для записи в историю).
    Done { full: String },
    /// Ошибка на любом этапе (retrieve/LLM); стрим завершается. `denied_kind` — типизированный
    /// отказ политики эгресса (AC-EGR-14: offline | feature | host) для i18n-рендера на фронте.
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        denied_kind: Option<&'static str>,
    },
}

/// Код отказа эгресса для фронта (AC-EGR-14); не-egress ошибки → `None` (генерик-рендер).
fn denied_code(e: &crate::ai::AiError) -> Option<&'static str> {
    use crate::net::EgressDenied;
    match e {
        crate::ai::AiError::Denied(EgressDenied::Offline) => Some("offline"),
        crate::ai::AiError::Denied(EgressDenied::FeatureNotEnabled(_)) => Some("feature"),
        crate::ai::AiError::Denied(EgressDenied::HostNotAllowed(_)) => Some("host"),
        _ => None,
    }
}

/// Код отказа для web-поиска (AC-EGR-14 + W4): секрет в запросе → "secret", отказ политики —
/// offline|feature|host, прочее — None (генерик-рендер).
fn web_denied_code(e: &crate::websearch::SearchError) -> Option<&'static str> {
    use crate::websearch::SearchError;
    match e {
        SearchError::SecretInQuery => Some("secret"),
        SearchError::Failed(m) if m.contains("офлайн") => Some("offline"),
        SearchError::Failed(m) if m.contains("не включена") => Some("feature"),
        SearchError::Failed(m) if m.contains("не разрешён") => Some("host"),
        _ => None,
    }
}

/// Кол-во RAG-чанков в контексте по умолчанию (калибруется eval-харнессом, Ф1-10).
const DEFAULT_K: usize = 8;

/// Чат со стримингом. `grounded` (по умолчанию `true`) — режим «по vault»: RAG-ретрив → источники →
/// промпт с контекстом. `grounded=false` — **общий чат** (V4.4): БЕЗ ретрива, ответ напрямую от
/// модели (источники пустые). Ответ стримится в `channel`.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // tauri-команда: web-режим добавил app+web к существующим
pub async fn chat_rag(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    channel: Channel<ChatStreamEvent>,
    question: String,
    k: Option<usize>,
    center: Option<String>,
    grounded: Option<bool>,
    web: Option<bool>,
) -> AppResult<()> {
    let grounded = grounded.unwrap_or(true);
    let web = web.unwrap_or(false);
    // Снимаем нужное из контекста и отпускаем лок ДО сетевых вызовов (эмбеддинг + LLM-стрим).
    let (reader, vectors, embedder, chat, chat_util) = {
        let ctx = state.vault().await?;
        (
            ctx.db.reader().clone(),
            ctx.vectors.clone(),
            ctx.ai.embedder.clone(),
            ctx.ai.chat.clone(),
            ctx.ai.chat_util.clone(),
        )
    };
    let Some(chat) = chat else {
        return Err("chat-провайдер не сконфигурирован (.nexus/local.json → ai.chat)".into());
    };

    // Web-агент (W-2): LLM решает «нужен интернет» → SearXNG → ответ с цитатами; результаты —
    // НЕДОВЕРЕННЫЙ контекст (anti-injection маркеры), tool-use запрещён, ≤MAX_SEARCHES/ход (W3).
    // Планировщик — мелкая модель (`chat_util`) либо сам `chat` при её отсутствии.
    let messages = if web {
        use crate::news::SystemResolver;
        use crate::websearch::{agent, config as web_config, WebSearcher};

        use tauri::Manager;
        let url = app
            .path()
            .app_config_dir()
            .ok()
            .map(|d| web_config::load(&d.join("websearch.json")))
            .unwrap_or_default()
            .url;
        let planner: &dyn crate::ai::ChatProvider = chat_util.as_deref().unwrap_or(chat.as_ref());
        let searcher = WebSearcher::new(
            state.egress_policy.clone(),
            state.egress_audit.clone(),
            std::sync::Arc::new(SystemResolver),
            url,
        );
        let cancel_plan = std::sync::Arc::new(AtomicBool::new(false));
        match agent::run(planner, &searcher, &question, &cancel_plan).await {
            Ok(outcome) if outcome.query.is_some() => {
                let _ = channel.send(ChatStreamEvent::WebSources {
                    sources: outcome.results.clone(),
                });
                let triples: Vec<(String, String, String)> = outcome
                    .results
                    .iter()
                    .map(|r| (r.title.clone(), r.url.clone(), r.snippet.clone()))
                    .collect();
                build_web_answer_messages(&question, &triples, &injection_marker())
            }
            Ok(_) => {
                // Модель решила, что интернет не нужен → общий чат (источников нет).
                let _ = channel.send(ChatStreamEvent::Sources {
                    sources: Vec::new(),
                });
                build_chat_messages(&question)
            }
            Err(e) => {
                let _ = channel.send(ChatStreamEvent::Error {
                    denied_kind: web_denied_code(&e),
                    message: e.to_string(),
                });
                return Ok(());
            }
        }
    } else if grounded {
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
                    denied_kind: None,
                });
                return Ok(());
            }
        };
        let _ = channel.send(ChatStreamEvent::Sources {
            sources: hits.clone(),
        });

        // 2) Контекст из полного содержимого чанков (в порядке релевантности).
        let ids: Vec<i64> = hits.iter().map(|h| h.chunk_id).collect();
        let texts = search::fetch_chunk_contexts(&reader, &ids).await?;
        let contexts: Vec<(String, String)> = hits
            .iter()
            .filter_map(|h| texts.get(&h.chunk_id).cloned())
            .collect();
        // Анти-инъекция (AC-SEC-7): обрамляем недоверенный контекст заметок случайным маркером запроса.
        build_rag_messages(&question, &contexts, &injection_marker())
    } else {
        // V4.4: общий чат — ретрив НЕ выполняется. Пустые источники, чтобы UI очистил прежние.
        let _ = channel.send(ChatStreamEvent::Sources {
            sources: Vec::new(),
        });
        build_chat_messages(&question)
    };

    // 3) Стриминг ответа (с отменой). Помечаем интерактивную LLM-операцию (S5): планировщик уступит
    // фоновые LLM-джобы, пока идёт чат.
    let _llm_busy = state.enter_interactive_llm();
    let cancel = state.begin_chat();

    // R1 — живой 💭-индикатор. gemma стримит размышление → копим в буфер + шлём сырые дельты (для
    // спойлера «развернуть»); ПАРАЛЛЕЛЬНО мелкая модель (`chat_util`) каждые ~1.5с суммаризует буфер в
    // короткую фразу (`ReasoningSummary`, обновляется живо). Отмена чата гасит и стрим, и суммаризатор.
    // Без `chat_util` — только сырой стрим reasoning (фраз нет).
    let reasoning = Arc::new(Mutex::new(String::new()));
    let done = Arc::new(AtomicBool::new(false));
    let summarizer = chat_util.clone().map(|util| {
        let (reasoning, done, cancel, channel) = (
            reasoning.clone(),
            done.clone(),
            cancel.clone(),
            channel.clone(),
        );
        tokio::spawn(async move {
            let mut last = 0usize;
            loop {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                let stop = done.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed);
                let text = reasoning.lock().map(|g| g.clone()).unwrap_or_default();
                if text.len() > last.saturating_add(40) {
                    last = text.len();
                    if let Ok(sum) = summarize_reasoning(&util, &text, &cancel).await {
                        if !sum.is_empty() {
                            let _ = channel.send(ChatStreamEvent::ReasoningSummary { text: sum });
                        }
                    }
                }
                if stop {
                    break;
                }
            }
        })
    });

    let result = {
        let mut on_token = |t: String| {
            let _ = channel.send(ChatStreamEvent::Token { text: t });
        };
        let mut on_reasoning = |t: String| {
            if let Ok(mut g) = reasoning.lock() {
                g.push_str(&t);
            }
            let _ = channel.send(ChatStreamEvent::Reasoning { text: t });
        };
        chat.stream_chat_reasoning(&messages, &mut on_token, &mut on_reasoning, &cancel)
            .await
    };
    done.store(true, Ordering::Relaxed);
    if let Some(h) = &summarizer {
        h.abort();
    }
    // Финальная сводка по ПОЛНОМУ размышлению (короткий CoT мог не успеть тикнуть в таске).
    if let Some(util) = &chat_util {
        let text = reasoning.lock().map(|g| g.clone()).unwrap_or_default();
        if !text.trim().is_empty() {
            if let Ok(sum) = summarize_reasoning(util, &text, &cancel).await {
                if !sum.is_empty() {
                    let _ = channel.send(ChatStreamEvent::ReasoningSummary { text: sum });
                }
            }
        }
    }

    match result {
        Ok(full) => {
            let _ = channel.send(ChatStreamEvent::Done { full });
        }
        Err(e) => {
            let _ = channel.send(ChatStreamEvent::Error {
                denied_kind: denied_code(&e),
                message: e.to_string(),
            });
        }
    }
    Ok(())
}

/// Суммаризует ход мысли в ОДНУ короткую фразу через мелкую модель (R1, `chat_util`). Берём хвост
/// размышления (последние ~2000 симв — самое свежее), просим короткую фразу настоящего времени.
/// Best-effort: ошибки гасятся вызывающим. Отмена чата прерывает и этот вызов (общий `cancel`).
async fn summarize_reasoning(
    util: &Arc<dyn ChatProvider>,
    reasoning: &str,
    cancel: &Arc<AtomicBool>,
) -> crate::ai::AiResult<String> {
    const TAIL: usize = 2000;
    let n = reasoning.chars().count();
    let tail: String = if n > TAIL {
        reasoning.chars().skip(n - TAIL).collect()
    } else {
        reasoning.to_string()
    };
    let messages = [
        ChatMessage::system(
            "По размышлению ассистента напиши ОДНУ очень короткую фразу (3–6 слов, настоящее время, \
             без точки и кавычек) — что он сейчас делает. Только фразу, по-русски.",
        ),
        ChatMessage::user(tail),
    ];
    let mut out = String::new();
    util.stream_chat(&messages, &mut |t| out.push_str(&t), cancel)
        .await?;
    Ok(out.trim().trim_matches('"').trim().to_string())
}

/// Отменяет активный чат-стрим (если есть). Идемпотентно.
#[tauri::command]
pub async fn chat_cancel(state: State<'_, AppState>) -> AppResult<()> {
    state.cancel_active_chat();
    Ok(())
}
