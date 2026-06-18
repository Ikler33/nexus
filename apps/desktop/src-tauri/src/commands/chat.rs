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
    build_agent_memory_block, build_chat_messages, build_episode_block, build_memory_block,
    build_pinned_block, build_rag_messages, build_web_answer_messages, injection_marker,
    prepend_memory_block, ChatMessage, ChatProvider,
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
    /// Память переписки (N4b): релевантные фрагменты прошлых диалогов (отдельный канал chat_vectors).
    /// Приходит до токенов, как и `Sources`; UI помечает их «из прошлых разговоров».
    MemorySources {
        sources: Vec<crate::chat_log::MemoryHit>,
    },
    /// Эпизодическая память (EP-2): саммари релевантных прошлых сессий (канал episode_vectors).
    /// Приходит до токенов; UI помечает «из прошлого разговора», по клику грузит сессию.
    EpisodeSources {
        sources: Vec<crate::episode::EpisodeHit>,
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
        SearchError::NotConfigured => Some("notConfigured"),
        SearchError::SecretInQuery => Some("secret"),
        SearchError::Failed(m) if m.contains("офлайн") => Some("offline"),
        SearchError::Failed(m) if m.contains("не включена") => Some("feature"),
        SearchError::Failed(m) if m.contains("не разрешён") => Some("host"),
        _ => None,
    }
}

/// Кол-во RAG-чанков в контексте по умолчанию (калибруется eval-харнессом, Ф1-10).
const DEFAULT_K: usize = 8;
/// Память переписки (N4b): сколько фрагментов прошлых диалогов подмешивать и до какой длины резать.
/// Консервативно мало — память это ФОН, а не основной контекст (не должна глушить заметки/ответ).
const MEMORY_K: usize = 3;
const MEMORY_SNIPPET_CHARS: usize = 280;
/// Эпизодическая память (EP-2): длина саммари эпизода при инъекции/показе (символы). Эпизод длиннее
/// сниппета переписки — но капается, чтобы не раздувать фон (порог K — `episode::EPISODE_K`).
const EPISODE_SNIPPET_CHARS: usize = 400;
/// Память агента (MEM, D2): сколько НЕ-пинов подмешивать по близости (пины — всегда, сверх этого).
/// Консервативно мало — факты это фон, как и переписка.
const AGENT_MEMORY_K: usize = 3;
/// P6-PIN: бюджет закреплённого контекста — макс. заметок и символов на заметку (защита окна модели).
const PINNED_MAX_NOTES: usize = 5;
const PINNED_NOTE_CHARS: usize = 4000;
/// Не читаем в RAM закреплённый файл крупнее этого (size-guard до read_to_string).
const PINNED_MAX_BYTES: u64 = 1_000_000;

/// P6-PIN БЕЗОПАСНОСТЬ: можно ли читать путь как закреплённую заметку в контекст ИИ. Только
/// `.md` и БЕЗ dot-компонентов (`.nexus` с секретами/local.json/БД заметок nexus.db/историей
/// чатов/vectors, `.git`). `resolve_vault_path` отсекает побег НАРУЖУ vault, но НЕ служебный
/// `.nexus` — он физически внутри root; без этого гарда битый/злонамеренный IPC-вызов с
/// `pinned=[".nexus/nexus.db"]` утёк бы секреты в LLM-канал (тот же явный guard, что в
/// delete_path/rename_path).
///
/// ИНВАРИАНТ: `is_pinnable` НЕ самодостаточен против traversal — `..`-сегменты он пропускает
/// СОЗНАТЕЛЬНО, полагаясь на последующий `resolve_vault_path` (canonicalize + проверка внутри root).
/// Звать ТОЛЬКО в паре с `resolve_vault_path`.
fn is_pinnable(path: &str) -> bool {
    path.to_lowercase().ends_with(".md")
        && !std::path::Path::new(path).components().any(|c| {
            matches!(c, std::path::Component::Normal(s) if s.to_string_lossy().starts_with('.'))
        })
}

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
    rerank: Option<bool>,
    memory: Option<bool>,
    agent_memory: Option<bool>,
    episodic: Option<bool>,
    deep: Option<bool>,
    session_id: Option<i64>,
    pinned: Option<Vec<String>>,
) -> AppResult<()> {
    let grounded = grounded.unwrap_or(true);
    let web = web.unwrap_or(false);
    // LLM-реранк источников (search::rerank, eval-гейт пройден): по умолчанию ВКЛ при наличии
    // утилитарной модели; тумблер — в настройках AI фронта.
    let rerank = rerank.unwrap_or(true);
    // Память переписки (N4b) — отдельный канал (chat_vectors), ВКЛ по умолчанию (решение владельца:
    // переписка часть «второго мозга»). Тумблер `aiChatMemory` в настройках AI фронта.
    let memory = memory.unwrap_or(true);
    // Память агента (MEM, явные факты) — ВЫКЛ по умолчанию (D5: приватность-first). Тумблер
    // `aiAgentMemory` в Настройках → AI (MEM-4). Отдельный канал (memory_vectors), не трогает RAG.
    let agent_memory = agent_memory.unwrap_or(false);
    // Эпизодическая память (EP-2) — саммари прошлых сессий (канал episode_vectors). ВЫКЛ по умолчанию
    // (приватность-first, как MEM); per-call флаг от фронта (тоггл `aiEpisodicMemory`). UI-тоггл — EP-3.
    let episodic = episodic.unwrap_or(false);
    // Глубокие размышления в чате (reasoning gemma, `chat`) vs «Быстрый» без CoT (`chat_fast`). ВЫКЛ
    // по умолчанию — «Быстрый» (тоггл `aiChatDeep` в Настройках → AI). Замер 2026-06-18: на RAG-по-базе
    // reasoning добавляет 30–40с БЕЗ выигрыша в качестве (модель переписывает один вывод 3×), потому
    // дефолт Быстрый; Глубокий остаётся доступен тогглом «на всякий случай» (решение владельца).
    let deep = deep.unwrap_or(false);
    // Снимаем нужное из контекста и отпускаем лок ДО сетевых вызовов (эмбеддинг + LLM-стрим).
    let (
        root,
        reader,
        writer,
        vectors,
        chat_vectors,
        memory_vectors,
        episode_vectors,
        embedder,
        chat,
        chat_fast,
        chat_util,
    ) = {
        let ctx = state.vault().await?;
        (
            ctx.root.clone(),
            ctx.db.reader().clone(),
            ctx.db.writer().clone(),
            ctx.vectors.clone(),
            ctx.chat_vectors.clone(),
            ctx.memory_vectors.clone(),
            ctx.episode_vectors.clone(),
            ctx.ai.embedder.clone(),
            ctx.ai.chat.clone(),
            ctx.ai.chat_fast.clone(),
            ctx.ai.chat_util.clone(),
        )
    };
    // Тоггл «Быстрый/Глубокий» (дефолт Быстрый): Быстрый → `chat_fast` (gemma БЕЗ reasoning),
    // Глубокий → `chat` (С reasoning). Оба на ОСНОВНОЙ модели, разница только в `enable_thinking`.
    // Fallback `.or(...)`: если нужной половины пары нет (напр. без `ai.fast`-секции), берём что есть —
    // чат не должен падать из-за выбора режима.
    let chat = if deep {
        chat.or(chat_fast)
    } else {
        chat_fast.or(chat)
    };
    let Some(chat) = chat else {
        return Err("chat-провайдер не сконфигурирован (.nexus/local.json → ai.chat)".into());
    };

    // Web-агент (W-2): LLM решает «нужен интернет» → SearXNG → ответ с цитатами; результаты —
    // НЕДОВЕРЕННЫЙ контекст (anti-injection маркеры), tool-use запрещён, ≤MAX_SEARCHES/ход (W3).
    // Планировщик — мелкая модель (`chat_util`) либо сам `chat` при её отсутствии.
    // Web — ДОПОЛНИТЕЛЬНЫЙ флаг к режиму (ревизия владельца 11.06): модель «может сходить в
    // интернет». План→поиск; если веб не нужен/пуст — отвечаем в ВЫБРАННОМ режиме (vault→RAG,
    // general→общий), а не принудительно в общем.
    let web_messages = if web {
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
                Some(build_web_answer_messages(
                    &question,
                    &triples,
                    &injection_marker(),
                ))
            }
            Ok(_) => None, // веб не нужен → ниже отвечаем в выбранном режиме
            Err(e) => {
                let _ = channel.send(ChatStreamEvent::Error {
                    denied_kind: web_denied_code(&e),
                    message: e.to_string(),
                });
                return Ok(());
            }
        }
    } else {
        None
    };
    let mut messages = if let Some(m) = web_messages {
        m
    } else if grounded {
        let k = k.unwrap_or(DEFAULT_K).clamp(1, 20);
        // Реранк активен при включённом флаге И доступной мелкой модели: ретрив глубже (топ-24),
        // LLM переупорядочивает, дальше берём k. Гейт: nDCG .883→1.0, MRR .848→1.0 на golden.
        let do_rerank = rerank && chat_util.is_some();
        // 1) Retrieve: гибридный поиск (с граф-рангом от открытого файла, если задан) → источники.
        let opts = SearchOptions {
            limit: if do_rerank {
                search::rerank::RERANK_RETRIEVE
            } else {
                k
            },
            filter: None,
            center,
        };
        let mut hits = match search::hybrid_search(
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
        if do_rerank {
            let util = chat_util.as_ref().expect("do_rerank ⇒ util");
            let rerank_cancel = std::sync::Arc::new(AtomicBool::new(false));
            hits = search::rerank::llm_rerank(util.as_ref(), &question, hits, &rerank_cancel).await;
            hits.truncate(k);
        }
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

    // EP-2: эпизодическая память — саммари релевантных ЗАВЕРШЁННЫХ сессий (канал episode_vectors,
    // ключи = id эпизодов). Отдельный канал, как N4b/MEM — note-RAG не трогает. Считаем ПЕРВЫМ, чтобы
    // дедуплицировать с N4b: если сессия всплыла эпизодом, её сырые реплики не дублируем (real_concern
    // дизайна). Текущую сессию исключаем. Best-effort. Дефолт OFF (флаг `aiEpisodicMemory`).
    let mut episode_session_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    if episodic {
        if let (Some(embedder), Some(episode_vectors)) =
            (embedder.as_ref(), episode_vectors.as_ref())
        {
            match crate::episode::search_episodes(
                &reader,
                episode_vectors,
                embedder.as_ref(),
                &question,
                crate::episode::EPISODE_K,
                session_id,
                EPISODE_SNIPPET_CHARS,
            )
            .await
            {
                Ok(hits) if !hits.is_empty() => {
                    episode_session_ids = hits.iter().map(|h| h.session_id).collect();
                    let _ = channel.send(ChatStreamEvent::EpisodeSources {
                        sources: hits.clone(),
                    });
                    let snippets: Vec<(String, String)> = hits
                        .iter()
                        .map(|h| {
                            (
                                format!("Прошлый разговор «{}»", h.session_title),
                                h.summary_snippet.clone(),
                            )
                        })
                        .collect();
                    prepend_memory_block(
                        &mut messages,
                        build_episode_block(&snippets, &injection_marker()),
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "episodic-memory: поиск эпизодов не удался"),
            }
        }
    }

    // N4b: память переписки — отдельный КАНАЛ (chat_vectors), не трогает note-RAG ранжирование
    // (eval-гейт держится: hybrid_search не задействован). Подмешиваем как фон к user-сообщению
    // ЛЮБОГО режима (vault/общий/web). Текущую сессию исключаем (не пересказываем сам себе), а также
    // сессии, уже всплывшие ЭПИЗОДОМ (EP-2 дедуп: один разговор не пересказан И процитирован).
    // Best-effort: ошибка эмбеддинга/поиска не валит чат — просто без памяти.
    if memory {
        if let (Some(embedder), Some(chat_vectors)) = (embedder.as_ref(), chat_vectors.as_ref()) {
            match crate::chat_log::search_memory(
                &reader,
                chat_vectors,
                embedder.as_ref(),
                &question,
                MEMORY_K,
                session_id,
                episode_session_ids.clone(),
                MEMORY_SNIPPET_CHARS,
            )
            .await
            {
                Ok(hits) if !hits.is_empty() => {
                    let _ = channel.send(ChatStreamEvent::MemorySources {
                        sources: hits.clone(),
                    });
                    let snippets: Vec<(String, String)> = hits
                        .iter()
                        .map(|h| {
                            let who = if h.role == "user" {
                                "вы"
                            } else {
                                "ассистент"
                            };
                            (
                                format!("Диалог «{}» ({who})", h.session_title),
                                h.snippet.clone(),
                            )
                        })
                        .collect();
                    prepend_memory_block(
                        &mut messages,
                        build_memory_block(&snippets, &injection_marker()),
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "chat-memory: поиск памяти не удался"),
            }
        }
    }

    // MEM (AC-MEM-5): память агента — курируемые ЯВНЫЕ ФАКТЫ (пины «всегда» + top-k близких), отдельный
    // КАНАЛ (memory_vectors + таблица memory_facts), не трогает note-RAG ранжирование (eval-гейт держится).
    // ВЫКЛ по умолчанию (D5), включается тумблером `aiAgentMemory`. Подмешиваем фоном к user-сообщению
    // ЛЮБОГО режима. Best-effort: ошибка эмбеддинга/поиска не валит чат — просто без памяти.
    if agent_memory {
        if let (Some(embedder), Some(memory_vectors)) = (embedder.as_ref(), memory_vectors.as_ref())
        {
            match crate::memory::context_facts(
                &reader,
                &writer,
                memory_vectors,
                embedder.as_ref(),
                &question,
                AGENT_MEMORY_K,
            )
            .await
            {
                Ok(facts) if !facts.is_empty() => {
                    let snippets: Vec<(String, String)> = facts
                        .iter()
                        .map(|f| {
                            let label = if f.pinned {
                                "Закреплённый факт"
                            } else {
                                "Факт"
                            };
                            (label.to_string(), f.text.clone())
                        })
                        .collect();
                    prepend_memory_block(
                        &mut messages,
                        build_agent_memory_block(&snippets, &injection_marker()),
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "agent-memory: поиск фактов не удался"),
            }
        }
    }

    // P6-PIN: закреплённые заметки — ГАРАНТИРОВАННЫЙ контекст (полное содержимое), не зависит от
    // RAG-ретрива. Безопасное чтение (resolve_vault_path: анти-traversal, как read_file_meta),
    // обрезка по бюджету, обёртка анти-инъекцией. Применяется в ЛЮБОМ режиме (vault/общий/web) — пин
    // = «обсудить ЭТИ заметки». Best-effort: битый/пропавший путь молча пропускается.
    if let Some(paths) = pinned.as_ref().filter(|p| !p.is_empty()) {
        let mut notes: Vec<(String, String)> = Vec::new();
        for path in paths.iter() {
            // Бюджет (PINNED_MAX_NOTES) считаем по УСПЕШНО добавленным, а не по входным позициям:
            // непинуемые/битые/служебные пути в начале списка не должны съедать слоты гарантированного
            // контекста (аудит 2026-06-18).
            if notes.len() >= PINNED_MAX_NOTES {
                break;
            }
            if !is_pinnable(path) {
                continue; // только .md-заметки, без служебных dot-путей (.nexus/.git) — см. is_pinnable
            }
            let Ok(abs) = crate::vault::resolve_vault_path(&root, std::path::Path::new(path))
            else {
                continue; // путь вне vault / битый — пропускаем (анти-traversal)
            };
            // БЕЗОПАСНОСТЬ (security-MAJOR, аудит 2026-06-18): `is_pinnable` — ЛЕКСИЧЕСКАЯ проверка
            // строки; `resolve_vault_path` канонизирует и ловит только побег НАРУЖУ root, но НЕ служебный
            // `.nexus` (он физически внутри root). Симлинк `notes/leak.md → ../.nexus/local.json` прошёл
            // бы оба и утёк бы секреты/историю чатов в LLM-канал. Сверяем КАНОНИЗИРОВАННЫЙ таргет через
            // `is_ignored` (паритет с `attachments.rs::safe_attachment_abs`): .nexus/.git/dotfile/*.db →
            // пропуск.
            if crate::watcher::is_ignored(&abs) {
                continue;
            }
            // Size-guard: не грузим в RAM огромный файл целиком (read_to_string читает всё ДО
            // обрезки PINNED_NOTE_CHARS). Слишком большой → пропускаем.
            if let Ok(meta) = tokio::fs::metadata(&abs).await {
                if meta.len() > PINNED_MAX_BYTES {
                    continue;
                }
            }
            match tokio::fs::read_to_string(&abs).await {
                Ok(mut text) => {
                    if text.chars().count() > PINNED_NOTE_CHARS {
                        text = text.chars().take(PINNED_NOTE_CHARS).collect::<String>()
                            + "\n…(обрезано)";
                    }
                    notes.push((format!("Закреплённая заметка: {path}"), text));
                }
                Err(e) => {
                    tracing::warn!(error = %e, %path, "pin: чтение закреплённой заметки не удалось")
                }
            }
        }
        prepend_memory_block(
            &mut messages,
            build_pinned_block(&notes, &injection_marker()),
        );
    }

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
            "Ты — живой индикатор хода мысли ассистента. По хвосту его размышления назови \
             КОНКРЕТНО, над чем он сейчас работает: предмет, вариант, сравнение или вывод — одной \
             фразой 5–10 слов, по-русски, настоящее время, без точки и кавычек. Пустые обобщения \
             запрещены («анализирую запрос», «обдумываю варианты», «изучаю вопрос») — называй \
             суть: «Сравниваю б/у 3090 и серверные P40», «Считаю VRAM под 70B-модель». Только фразу.",
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

#[cfg(test)]
mod tests {
    use super::is_pinnable;

    #[test]
    fn pinnable_allows_md_blocks_service_paths() {
        // Обычные .md-заметки — можно.
        assert!(is_pinnable("Inbox.md"));
        assert!(is_pinnable("Projects/Roadmap.md"));
        assert!(is_pinnable("Заметки/Идея.MD")); // регистр расширения не важен
                                                 // Служебные dot-пути (секреты/БД/история) и не-.md — НЕЛЬЗЯ (анти-эксфильтрация в LLM).
        assert!(!is_pinnable(".nexus/local.json"));
        assert!(!is_pinnable(".nexus/nexus.db"));
        assert!(!is_pinnable(".nexus/history/x.md")); // .md, но dot-компонент .nexus
        assert!(!is_pinnable(".git/config"));
        assert!(!is_pinnable("Notes/.hidden.md")); // dot-файл
        assert!(!is_pinnable("README.txt")); // не .md
        assert!(!is_pinnable("image.png"));
    }
}
