//! Команды vault: открытие хранилища и ленивый листинг каталогов.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::OptionalExtension;
use tauri::{Manager, State};

use crate::ai::{
    self, AIClient, ChatConfig, ChatProvider, EmbeddingProvider, LocalConfig, OpenAiChatProvider,
    OpenAiEmbedder,
};
use crate::db::Database;
use crate::error::{AppError, AppResult};
use crate::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient};
use crate::state::{AppState, VaultContext};
use crate::vault::{self, FileEntry, FileMeta, NoteRef, VaultInfo};
use crate::vector::VectorIndex;

/// Открывает vault: канонизирует папку, открывает БД в `.nexus/nexus.db`, сохраняет в state.
#[tauri::command]
pub async fn open_vault(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    path: String,
) -> AppResult<VaultInfo> {
    let root = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| AppError::Msg(format!("vault path: {e}")))?;
    if !root.is_dir() {
        return Err("vault: путь не является каталогом".into());
    }

    let db = Database::open(root.join(".nexus").join("nexus.db")).await?;

    // P0-b: подключаем durable-сток egress-audit ПОСЛЕ открытия БД (журнал строится в AppState ДО
    // vault). С этого момента весь реальный эгресс (chat/embed/probe) durable-аудитится write-before-act.
    state.egress_audit.set_writer(db.writer().clone());

    let info = VaultInfo {
        root: root.to_string_lossy().into_owned(),
        name: vault::vault_name(&root),
    };

    // Конфиг `.nexus/local.json` парсим ОДИН раз (раньше — дважды: build_rag + build_chat), кросс-план #8.
    let local_cfg = load_local_config(&root).await;

    // W-3: глобальный web-consent (`websearch.json`) грузим ЗАРАНЕЕ (пока `app` не перемещён) — в конце
    // зеркалим его в `ai.web` ЭТОГО vault (см. ниже).
    let web_consent = app
        .path()
        .app_config_dir()
        .ok()
        .map(|d| crate::websearch::config::load(&d.join("websearch.json")));

    // Авто-allowlist эгресса (ADR-005-ext E4): хосты явных `ai.*.url` из local.json. Нет конфига →
    // пусто (fail-closed для публичных хостов; LAN/loopback живут как `is_private_host`).
    state.egress_policy.set_allowlist(
        local_cfg
            .as_ref()
            .map(LocalConfig::egress_hosts)
            .unwrap_or_default(),
    );

    // RAG (Ф1-5): строим эмбеддер + векторный индекс. Если конфига нет / нет embedding-секции /
    // сервер недоступен — vault открывается без AI (local-first).
    let rag = match &local_cfg {
        Some(cfg) => build_rag(&root, &db, cfg, &state.egress_policy, &state.egress_audit).await,
        None => None,
    };
    let (vectors, chat_vectors, memory_vectors, episode_vectors, embedder, indexer) = match rag {
        Some((embedder, vec_index, chat_vec_index, mem_vec_index, ep_vec_index, force)) => {
            let idx = crate::indexer::Indexer::with_rag(
                &db,
                root.clone(),
                embedder.clone(),
                vec_index.clone(),
                force,
            );
            (
                Some(vec_index),
                Some(chat_vec_index),
                Some(mem_vec_index),
                Some(ep_vec_index),
                Some(embedder),
                idx,
            )
        }
        None => (
            None,
            None,
            None,
            None,
            None,
            crate::indexer::Indexer::new(&db, root.clone()),
        ),
    };

    // Chat-провайдеры (ADR-005): пара — обычный с reasoning (RAG-чат, точность) + «быстрый» без
    // reasoning (примитивы R2: inline/дайджест/судья). Строятся вместе (есть/нет синхронно).
    let (chat, chat_fast) = match &local_cfg {
        Some(cfg) => match build_chat(cfg, &state.egress_policy, &state.egress_audit).await {
            Some((normal, fast)) => (Some(normal), Some(fast)),
            None => (None, None),
        },
        None => (None, None),
    };
    // Утилитарная мелкая модель (`ai.fast`, напр. Qwen3-4B :8084) для коротких примитивов (inline/судья).
    // Нет секции `ai.fast` → fallback на gemma-fast (chat_fast), чтобы ничего не сломалось.
    let chat_util = match &local_cfg {
        Some(cfg) => build_util_chat(cfg, &state.egress_policy, &state.egress_audit),
        None => None,
    }
    .or_else(|| chat_fast.clone());

    // Запускаем watcher + фоновую индексацию (начальный скан + инкрементальные события).
    // Watcher живёт в VaultContext::lifecycle: его дроп (повторный open_vault) гасит петлю.
    // Sender — управляющий вход той же петли для `rescan_vault` (VaultEvent::Rescan).
    // CORE-1c-1: индексатор tauri-free — проводка Tauri-эвентов теперь ЗДЕСЬ. Строим
    // `IndexerHooks` из `AppHandle` (прогресс/индекс-обновлён/файл-изменён) и инъектируем в ядро.
    let (watcher, index_tx) = match crate::indexer::spawn(indexer, indexer_hooks(app.clone())) {
        Some((w, tx)) => (Some(w), Some(tx)),
        None => (None, None),
    };

    // Планировщик фоновых задач (ADR-007): встроенный kind `gc` (самоочистка) + (если есть chat) `digest`
    // (LLM-дайджест недавних изменений, #35). Воркер живёт, пока открыт vault.
    let mut registry = crate::scheduler::default_registry(db.writer().clone());
    // HOME-виджеты (H2-фундамент): реестр `key → kind планировщика, который его генерирует` — по нему
    // `refresh_widget` ставит джобу и дедупит. Наполняется ниже (H3+); пуст, если LLM не сконфигурирован.
    let mut widget_registry = crate::home::widgets::WidgetRegistry::new();
    // Сток событий HOME-виджетов (H2: `home:widget-updated`) — один на vault, шарится дайджестом (H3) и
    // stale-radar (H4). Конструируется всегда; используется только зарегистрированными виджетами.
    let widget_sink: Arc<dyn crate::home::widgets::WidgetSink> =
        Arc::new(crate::home::widgets::TauriWidgetSink(app.clone()));
    // Дайджест/судья — это примитивы: берут «быстрый» chat без reasoning (R2).
    if let Some(fast) = &chat_fast {
        // H3: дайджест недавних изменений — это HOME-виджет «Daily brief» (зона 2, on-open). После
        // генерации дайджест зеркалится в кэш `home_widgets` + событие `home:widget-updated`; виджет
        // бэкает тот же kind `digest` (одна генерация на обе поверхности — панель дайджеста и HOME).
        let handler: Arc<dyn crate::scheduler::JobHandler> = Arc::new(
            crate::digest::DigestHandler::new(
                db.reader().clone(),
                fast.clone(),
                db.writer().clone(),
            )
            .with_home_widget(widget_sink.clone()),
        );
        registry.insert(crate::digest::KIND_DIGEST.to_string(), handler);
        widget_registry.register(crate::digest::KEY_DAILY_BRIEF, crate::digest::KIND_DIGEST);
        // Бутстрап: показать последний дайджест в виджете сразу на открытии (до следующей генерации).
        let _ = crate::digest::mirror_latest_to_widget(db.reader(), db.writer()).await;
    }
    // «Поиск противоречий» (#vision) — судья: короткие пары → утилитарная модель (chat_util). Нужны векторы.
    if let (Some(util), Some(vectors)) = (&chat_util, &vectors) {
        let handler: Arc<dyn crate::scheduler::JobHandler> =
            Arc::new(crate::contradictions::ContradictionHandler::new(
                db.reader().clone(),
                vectors.clone(),
                util.clone(),
                db.writer().clone(),
            ));
        registry.insert(crate::contradictions::KIND_CONTRA.to_string(), handler);
    }
    // «Stale radar» (H4) — слой 2: LLM-обогащение топ-N устаревших заметок (причина/действие/подсказка).
    // Судья-подобный примитив → утилитарная `chat_util`. AIP-хвост: теперь ПРОАКТИВЕН (recurring раз/сутки
    // + сид-if-needs_enrichment на открытии, ниже), как open_questions; команда `refresh_stale_radar` —
    // ручной триггер сверх того.
    if let Some(util) = &chat_util {
        let handler: Arc<dyn crate::scheduler::JobHandler> =
            Arc::new(crate::home::stale::StaleRadarHandler::new(
                db.reader().clone(),
                util.clone(),
                db.writer().clone(),
                widget_sink.clone(),
            ));
        registry.insert(crate::home::stale::KIND_STALE.to_string(), handler);
    }
    // HOME LLM-виджеты на фреймворке H2 (`WidgetHandler`: генерация → кэш `home_widgets` → событие).
    // Open questions (H5, зона 4, manual): короткое извлечение незакрытых вопросов → утилитарная `chat_util`.
    if let Some(util) = &chat_util {
        let key = crate::home::insights::KEY_OPEN_QUESTIONS;
        let kind = crate::home::widgets::widget_kind(key);
        let generator: Arc<dyn crate::home::widgets::WidgetGenerator> = Arc::new(
            crate::home::insights::OpenQuestionsGenerator::new(db.reader().clone(), util.clone()),
        );
        let handler: Arc<dyn crate::scheduler::JobHandler> =
            Arc::new(crate::home::widgets::WidgetHandler::new(
                key,
                generator,
                widget_sink.clone(),
                db.reader().clone(),
                db.writer().clone(),
                true,
            ));
        registry.insert(kind.clone(), handler);
        widget_registry.register(key, &kind);
    }
    // Context drift (H5, зона 5, scheduled): сравнение фокуса и целей — больше контекста → `chat_fast`/gemma.
    if let Some(fast) = &chat_fast {
        let key = crate::home::insights::KEY_CONTEXT_DRIFT;
        let kind = crate::home::widgets::widget_kind(key);
        let generator: Arc<dyn crate::home::widgets::WidgetGenerator> = Arc::new(
            crate::home::insights::ContextDriftGenerator::new(db.reader().clone(), fast.clone()),
        );
        let handler: Arc<dyn crate::scheduler::JobHandler> =
            Arc::new(crate::home::widgets::WidgetHandler::new(
                key,
                generator,
                widget_sink.clone(),
                db.reader().clone(),
                db.writer().clone(),
                true,
            ));
        registry.insert(kind.clone(), handler);
        widget_registry.register(key, &kind);
    }
    // Эпизодическая память (EP-1): фоновая суммаризация «созревших» чат-сессий в эпизоды. Примитив-
    // суммаризация → утилитарная `chat_util` (с фолбэком на gemma-fast). Гейт по `chat_util` + persisted-
    // тоггл `episodic.enabled` (handler рано выходит NOOP при OFF). Эмбеддер/индекс — для вектора саммари.
    if let Some(util) = &chat_util {
        let handler: Arc<dyn crate::scheduler::JobHandler> =
            Arc::new(crate::episode::EpisodeRollupHandler::new(
                db.reader().clone(),
                db.writer().clone(),
                util.clone(),
                embedder.clone(),
                episode_vectors.clone(),
            ));
        registry.insert(crate::episode::KIND_EPISODE_ROLLUP.to_string(), handler);
    }
    // Лента новостей (NF-4, AC-NF-6/7): хендлер прогона — guarded-фетчер (NewsFeed-фича,
    // DNS-гард) + утилитарная модель (примитив без reasoning). Регистрируем при наличии LLM;
    // конфиг news.json хендлер перечитывает на каждый прогон (выключено → no-op, consent).
    let news_config_path = {
        use tauri::Manager;
        app.path()
            .app_config_dir()
            .ok()
            .map(|d| d.join("news.json"))
    };
    // W-40: регистрируем при наличии ХОТЯ БЫ одной модели; выбор util(ai.fast)/main(ai.chat) — на
    // КАЖДЫЙ прогон по news.json::model_pref (горячее переключение в NewsFeedHandler::handle).
    let has_news_model = chat_util.is_some() || chat_fast.is_some();
    let news_active =
        if let Some(config_path) = news_config_path.as_ref().filter(|_| has_news_model) {
            let handler: Arc<dyn crate::scheduler::JobHandler> =
                Arc::new(crate::news::NewsFeedHandler {
                    fetcher: Arc::new(crate::news::GuardedNewsFetcher::new(
                        state.egress_policy.clone(),
                        state.egress_audit.clone(),
                        Arc::new(crate::news::SystemResolver),
                    )),
                    chat_util: chat_util.clone(),
                    chat_fast: chat_fast.clone(),
                    writer: db.writer().clone(),
                    reader: db.reader().clone(),
                    config_path: config_path.clone(),
                    // W-2/W-40: URL утилитарной (ai.fast) и основной (ai.chat) — для видимой ошибки
                    // в сводке прогона по ВЫБРАННОЙ модели при недостижимости.
                    url_util: local_cfg
                        .as_ref()
                        .and_then(|c| c.ai.fast.as_ref().map(|f| f.url.clone())),
                    url_fast: local_cfg
                        .as_ref()
                        .and_then(|c| c.ai.chat.as_ref().map(|ch| ch.url.clone())),
                    progress: {
                        // Этапы прогона → UI (фидбэк 11.06: живой статус «Опрашиваю источники 7/16»
                        // вместо немого «Собираю…»). Best-effort, как jobs:changed.
                        let app = app.clone();
                        Arc::new(move |stage: &str, done: usize, total: usize| {
                            use tauri::Emitter;
                            let _ = app.emit(
                                "news:progress",
                                serde_json::json!({ "stage": stage, "done": done, "total": total }),
                            );
                        })
                    },
                });
            registry.insert(crate::news::KIND_NEWSFEED.to_string(), handler);
            true
        } else {
            false
        };

    // Recurring (slice 6): LLM-фичи сами переназначаются после прогона — авто-обновление раз в сутки
    // (совпадает с их окном «недавнего»). С backpressure (S5) фон не мешает интерактиву.
    const DAY_SECS: i64 = 24 * 3600;
    let mut recurring: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    // Тогглы фоновых ИИ-фич (persisted в settings vault, дефолт OFF — real-test 2026-06-18): инсайты
    // (open_questions/context_drift/stale) и поиск противоречий при OFF НЕ регистрируем в recurring и НЕ
    // сидим ниже (фон/LLM не тратится; хендлеры дополнительно рано выходят NOOP при mid-session OFF).
    let insights_on = crate::home::insights::insights_enabled(db.reader()).await;
    let contra_on = crate::contradictions::is_enabled(db.reader()).await;
    if chat.is_some() {
        recurring.insert(crate::digest::KIND_DIGEST.to_string(), DAY_SECS);
    }
    if chat.is_some() && vectors.is_some() && contra_on {
        recurring.insert(crate::contradictions::KIND_CONTRA.to_string(), DAY_SECS);
    }
    // On-change (slice 7): дайджест+противоречия перезапускаются после правок vault (с дебаунсом).
    let on_change: Vec<String> = recurring.keys().cloned().collect();
    // Context drift (H5) — scheduled-only (раз/сутки; концепт: «чаще нет смысла»): в `recurring`, но НЕ в
    // `on_change` — добавляем ПОСЛЕ снятия on_change, чтобы он не реагировал на каждую правку.
    if chat.is_some() && insights_on {
        recurring.insert(
            crate::home::widgets::widget_kind(crate::home::insights::KEY_CONTEXT_DRIFT),
            DAY_SECS,
        );
    }
    // Open questions (H5) — AIP-5: проактивно раз/сутки (как context drift), scheduled-only (НЕ on-change,
    // добавлено после снятия on_change). Раньше — manual-only; теперь генерируется само, чтобы карточка
    // не висела пустой с «нажми обновить». Хендлер на `chat_util`, поэтому и гейт по нему.
    if chat_util.is_some() && insights_on {
        recurring.insert(
            crate::home::widgets::widget_kind(crate::home::insights::KEY_OPEN_QUESTIONS),
            DAY_SECS,
        );
    }
    // Stale radar (H4) — AIP-хвост: слой 2 теперь ПРОАКТИВЕН (раз/сутки, scheduled-only, как
    // open_questions; добавлено после снятия on_change — правка делает заметку МЕНЕЕ устаревшей, спешить
    // с переобогащением незачем). Per-note кэш делает повторный прогон дешёвым (пропуск валидного).
    if chat_util.is_some() && insights_on {
        recurring.insert(crate::home::stale::KIND_STALE.to_string(), DAY_SECS);
    }
    // Эпизоды (EP) — scheduled-only (как context drift / open questions; добавлено ПОСЛЕ снятия
    // on_change, чтобы НЕ реагировать на каждую правку: эпизод — «успокаивающийся» сигнал, сессии
    // должны затихнуть). Чаще суток: ~6 ч, чтобы завершённые днём сессии суммировались тем же днём.
    if chat_util.is_some() {
        recurring.insert(
            crate::episode::KIND_EPISODE_ROLLUP.to_string(),
            DAY_SECS / 4,
        );
    }
    // Лента (D3): раз/сутки, НЕ on-change (сетевая, от правок vault не зависит); при выключенной
    // фиче прогон — дешёвый no-op хендлера.
    if news_active {
        recurring.insert(crate::news::KIND_NEWSFEED.to_string(), DAY_SECS);
    }
    // Воркер планировщика: spawner хранит конфиг (для ручного рестарта N1), хендл — в lifecycle.
    // Дроп sender'а в хендле (повторный open_vault / закрытие) гасит worker_loop (аудит 2026-06-10).
    let scheduler_spawner = crate::scheduler::WorkerSpawner {
        writer: db.writer().clone(),
        app,
        registry: Arc::new(registry),
        recurring,
        reader: db.reader().clone(),
        on_change,
    };
    let scheduler_worker = scheduler_spawner.start();
    // Бэкфилл памяти переписки (N4): сессии до N4 (или потерянные векторы) индексируем в фоне —
    // эмбеддим сообщения, которых нет в chat_vectors. Best-effort, не держит open_vault.
    if let (Some(chat_vec), Some(emb)) = (&chat_vectors, &embedder) {
        let (reader, chat_vec, emb) = (db.reader().clone(), chat_vec.clone(), emb.clone());
        tokio::spawn(async move {
            // usearch — источник правды о проиндексированном: берём все сообщения, фильтруем `contains`.
            let all = std::collections::HashSet::new();
            if let Ok(msgs) = crate::chat_log::messages_missing_vectors(&reader, all).await {
                let pending: Vec<_> = msgs
                    .into_iter()
                    .filter(|m| !chat_vec.contains(m.id as u64))
                    .collect();
                if pending.is_empty() {
                    return;
                }
                let n = pending.len();
                for m in pending {
                    if let Ok(v) = emb.embed_documents(&[m.content.as_str()]).await {
                        if let Some(vec) = v.first() {
                            let _ = chat_vec.upsert(m.id as u64, vec);
                        }
                    }
                }
                let _ = chat_vec.save();
                tracing::info!(
                    messages = n,
                    "chat-memory: бэкфилл векторов переписки завершён"
                );
            }
        });
    }
    // Бэкфилл эпизодической памяти (EP): эпизоды без вектора (RAG включился позже / смена эмбеддера
    // дропнула индекс §6) — эмбеддим summary в фоне. `contains` — источник правды (как chat_vectors).
    // Best-effort, не держит open_vault.
    if let (Some(ep_vec), Some(emb)) = (&episode_vectors, &embedder) {
        let (reader, ep_vec, emb) = (db.reader().clone(), ep_vec.clone(), emb.clone());
        tokio::spawn(async move {
            if let Ok(rows) = crate::episode::episodes_for_backfill(&reader).await {
                let pending: Vec<_> = rows
                    .into_iter()
                    .filter(|(id, _)| !ep_vec.contains(*id as u64))
                    .collect();
                if pending.is_empty() {
                    return;
                }
                let n = pending.len();
                for (id, summary) in pending {
                    if let Ok(v) = emb.embed_documents(&[summary.as_str()]).await {
                        if let Some(vec) = v.first() {
                            let _ = ep_vec.upsert(id as u64, vec);
                        }
                    }
                }
                let _ = ep_vec.save();
                tracing::info!(
                    episodes = n,
                    "episodic-memory: бэкфилл векторов эпизодов завершён"
                );
            }
        });
    }
    // Бэкфилл памяти агента (MEM, P1-4): импортированные из бэкапа факты пишутся в `memory_facts`, но
    // НЕ в `memory_vectors` (`import_backup` не сериализует/переэмбеддит вектора) → слепы для
    // семантического recall (orphan-дыра того же класса, что закрыл эпизодный бэкфилл выше). Эмбеддим
    // факты без вектора в фоне. `contains` — источник правды (как chat_vectors/episode_vectors).
    // Best-effort, не держит open_vault.
    if let (Some(mem_vec), Some(emb)) = (&memory_vectors, &embedder) {
        let (reader, mem_vec, emb) = (db.reader().clone(), mem_vec.clone(), emb.clone());
        tokio::spawn(async move {
            if let Ok(rows) = crate::memory::memory_facts_for_backfill(&reader).await {
                let pending: Vec<_> = rows
                    .into_iter()
                    .filter(|(id, _)| !mem_vec.contains(*id as u64))
                    .collect();
                if pending.is_empty() {
                    return;
                }
                let n = pending.len();
                for (id, text) in pending {
                    // `embed_query` (НЕ `embed_documents`): память СИММЕТРИЧНА — `index_fact` и recall
                    // `context_facts` оба эмбеддят query-путём. На bge-m3 (пустые префиксы) разницы нет,
                    // но nomic/e5 кладут query/document в РАЗНЫЕ субпространства → импортированный факт
                    // не совпал бы с тем же фактом, добавленным руками. (Эпизоды/чат асимметричны —
                    // там `embed_documents` для хранения верно.)
                    if let Ok(vec) = emb.embed_query(&text).await {
                        let _ = mem_vec.upsert(id as u64, &vec);
                    }
                }
                let _ = mem_vec.save();
                tracing::info!(facts = n, "agent-memory: бэкфилл векторов фактов завершён");
            }
        });
    }
    // Seed: gc на ближайший тик; дайджест — если просрочен (run-if-overdue, S2) и chat сконфигурирован.
    let _ = crate::scheduler::enqueue(db.writer(), crate::scheduler::KIND_GC, "", 0, 3).await;
    if chat.is_some()
        && crate::digest::should_generate(db.reader())
            .await
            .unwrap_or(false)
    {
        let _ = crate::scheduler::enqueue(db.writer(), crate::digest::KIND_DIGEST, "", 0, 2).await;
    }
    // Context drift + open questions (H5) — run-if-overdue на открытии (как дайджест), через H2
    // `is_overdue` (нет кэша ИЛИ vault менялся с прошлой генерации). AIP-5: open_questions теперь тоже
    // сидится проактивно (раньше был manual-only). drift — на `chat_fast`/`chat`, open_questions — на
    // `chat_util`; mtime считаем один раз на оба.
    {
        let mtime = crate::scheduler::max_file_mtime(db.reader())
            .await
            .unwrap_or(0);
        let mut seeds: Vec<&str> = Vec::new();
        if chat.is_some() && insights_on {
            seeds.push(crate::home::insights::KEY_CONTEXT_DRIFT);
        }
        if chat_util.is_some() && insights_on {
            seeds.push(crate::home::insights::KEY_OPEN_QUESTIONS);
        }
        for key in seeds {
            if crate::home::widgets::is_overdue(db.reader(), key, mtime)
                .await
                .unwrap_or(false)
            {
                let _ = crate::scheduler::enqueue(
                    db.writer(),
                    &crate::home::widgets::widget_kind(key),
                    "",
                    0,
                    2,
                )
                .await;
            }
        }
    }
    // Stale radar (H4) — AIP-хвост: проактивный сид на открытии. Гейт `needs_enrichment` (НЕ H2
    // `is_overdue` — stale не виджет `home_widgets`): обогащаем, только если среди топ-устаревших есть
    // НЕобогащённые/протухшие. Иначе при свежем кэше открытие не дёргало бы LLM зря.
    if chat_util.is_some()
        && insights_on
        && crate::home::stale::needs_enrichment(db.reader(), crate::scheduler::now_secs())
            .await
            .unwrap_or(false)
    {
        let _ =
            crate::scheduler::enqueue(db.writer(), crate::home::stale::KIND_STALE, "", 0, 2).await;
    }
    // Лента (D3 «при первом открытии за день»): сид run-if-overdue — фича включена и последний
    // прогон старше суток (или прогонов не было).
    if news_active
        && news_config_path
            .as_deref()
            .map(crate::news::load_news_config)
            .is_some_and(|c| c.enabled)
    {
        let now = crate::scheduler::now_secs();
        // НЕ `is_none_or` — стабилен с 1.82, MSRV проекта 1.77 (clippy::incompatible_msrv).
        let overdue = !matches!(
            crate::news::latest_run(db.reader()).await.ok().flatten(),
            Some(r) if r.run_at >= now - DAY_SECS
        );
        if overdue {
            let _ =
                crate::scheduler::enqueue(db.writer(), crate::news::KIND_NEWSFEED, "", 0, 2).await;
        }
    }
    // Поиск противоречий — run-if-overdue (нужны и chat, и векторы).
    if chat.is_some()
        && vectors.is_some()
        && contra_on
        && crate::contradictions::should_generate(db.reader())
            .await
            .unwrap_or(false)
        // B17: дедуп при повторном открытии vault — не ставим второй contra-джоб, если уже есть
        // готовая/выполняющаяся. has_ready_job (а НЕ reschedule_if_absent): будущая recurring-pending
        // НЕ должна блокировать немедленный overdue-запуск (находка аудита).
        && !crate::scheduler::has_ready_job(
            db.reader(),
            crate::contradictions::KIND_CONTRA,
            crate::scheduler::now_secs(),
        )
        .await
        .unwrap_or(false)
    {
        let _ =
            crate::scheduler::enqueue(db.writer(), crate::contradictions::KIND_CONTRA, "", 0, 2)
                .await;
    }
    // Эпизоды (EP) — сид run-if-overdue на открытии: тоггл ON, есть «созревшие» сессии без актуального
    // эпизода, и нет уже готовой/выполняющейся джобы (`has_ready_job`, как contra: будущая
    // recurring-pending НЕ блокирует немедленный overdue — урок B17/#63).
    if chat_util.is_some()
        && crate::episode::is_enabled(db.reader()).await
        && crate::episode::has_stale_episodes(db.reader(), crate::scheduler::now_secs())
            .await
            .unwrap_or(false)
        && !crate::scheduler::has_ready_job(
            db.reader(),
            crate::episode::KIND_EPISODE_ROLLUP,
            crate::scheduler::now_secs(),
        )
        .await
        .unwrap_or(false)
    {
        let _ =
            crate::scheduler::enqueue(db.writer(), crate::episode::KIND_EPISODE_ROLLUP, "", 0, 2)
                .await;
    }

    // CONN-2/CONN-4: выбор агент-бэкенда по `ai.connection.mode` (default embedded — нулевая регрессия).
    // Единый хелпер (тот же зовёт `set_agent_connection` при смене в UI). Делаем ДО перемещения `root`
    // в VaultContext (нужен для пути сокета). Lazy: соединение откроется на первом прогоне.
    *state.agent_backend.write().await =
        crate::agent_backend::select_agent_backend(local_cfg.as_ref(), &root);

    // Фасад §4.3 (AC-EGR-13): ВСЕ провайдеры + политика — одним полем; policy — тот же Arc, что
    // в AppState (один экземпляр на приложение, через него hot-swap пересоберёт guarded-клиент).
    *state.vault.write().await = Some(VaultContext {
        root,
        db,
        vectors,
        chat_vectors,
        memory_vectors,
        episode_vectors,
        ai: AIClient {
            chat,
            chat_fast,
            chat_util,
            embedder,
            // AGENT-1 (I-5): tool-capable провайдер НЕ на десктопе — конструируется только в nexus-agentd.
            agent_tools: None,
            policy: state.egress_policy.clone(),
        },
        widgets: Arc::new(widget_registry),
        index_tx,
        lifecycle: crate::state::VaultLifecycle {
            watcher,
            scheduler_spawner,
            scheduler_worker: std::sync::Mutex::new(scheduler_worker),
        },
    });
    // W-3: зеркалим ГЛОБАЛЬНЫЙ web-consent (`websearch.json`) → `ai.web` ИМЕННО ЭТОГО vault, чтобы
    // веб-инструменты агента включались и в только что открытом vault (а не только в том, где жали
    // тоггл) — иначе UI (глобальный) показывал бы «Веб ВКЛ», а у агента в этом vault веба нет
    // (рассинхрон consent/UI, ревью W-3 major). Симметрично синку egress-политики из consent-файлов
    // на старте (lib.rs). skip-if-equal — без лишних атомарных записей на каждое открытие. `ai.web`
    // не влияет на уже построенные chat/rag (агент читает local.json заново per-run), поэтому после.
    if let Some(web_cfg) = web_consent {
        let cur = local_cfg
            .as_ref()
            .and_then(|c| c.ai.web.as_ref())
            .map(|w| (w.enabled, w.url.as_str()));
        if crate::commands::settings::web_needs_mirror(cur, web_cfg.enabled, &web_cfg.url) {
            if let Err(e) = crate::commands::settings::mirror_web_to_vault(
                &state,
                web_cfg.enabled,
                &web_cfg.url,
            )
            .await
            {
                tracing::warn!(error = %e, "open_vault: не удалось синхронизировать ai.web из websearch.json");
            }
        }
    }

    tracing::info!(vault = %info.root, "opened vault");
    Ok(info)
}

// ── проводка индексатора к Tauri (CORE-1c-1) ─────────────────────────────────────────────────────
// Индексатор (nexus-core) tauri-free: watcher-петля зовёт инъектируемые `IndexerHooks`. Эмит-glue
// (payload-структуры + `AppHandle::emit`) живёт ЗДЕСЬ, в desktop-крейте, и собирается в хуки.

/// Payload события `vault:index-progress` (camelCase для фронта).
#[derive(serde::Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct IndexProgress {
    done: usize,
    total: usize,
}

/// Payload события `vault:file-changed` (SAFE-3): относительный путь + blake3-хеш текущего диска.
/// Фронт сверяет хеш с `Buffer.baseHash`: совпал → эхо своего сейва (игнор); расходится → тихий
/// reload (чистый буфер) либо баннер guard'а (грязный буфер). camelCase для фронта.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FileChanged {
    path: String,
    hash: String,
}

/// Строит [`crate::indexer::IndexerHooks`] из `AppHandle`: три best-effort Tauri-эвента
/// (прогресс полного скана / «индекс обновлён» / «файл на диске изменился»). Раньше эта glue жила
/// внутри `indexer::events` — после CORE-1c-1 индексатор tauri-free, проводка эвентов — в app.
fn indexer_hooks(app: tauri::AppHandle) -> crate::indexer::IndexerHooks {
    use tauri::Emitter;
    let progress_app = app.clone();
    let changed_app = app.clone();
    crate::indexer::IndexerHooks {
        // Прогресс полного скана → событие фронту (статусбар «Индексация N/M», макет app.jsx).
        on_progress: Arc::new(move |done, total| {
            let _ = progress_app.emit("vault:index-progress", IndexProgress { done, total });
        }),
        // «Индекс vault обновлён» (ADR-007 S8): фронт перечитывает зависимые вьюхи (напр. «Цели» #35).
        on_vault_changed: Arc::new(move || {
            let _ = app.emit("vault:changed", ());
        }),
        // «Конкретный файл на диске изменился» (SAFE-3): фронт решает судьбу открытого буфера пути.
        on_file_changed: Arc::new(move |path, hash| {
            let _ = changed_app.emit("vault:file-changed", FileChanged { path, hash });
        }),
    }
}

/// Читает и парсит `.nexus/local.json` ОДИН раз (кросс-план #8 — раньше парсили дважды). `None` —
/// конфига нет / битый JSON (AI отключается, vault работает без AI — local-first).
async fn load_local_config(root: &Path) -> Option<LocalConfig> {
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .ok()?;
    LocalConfig::parse(&raw)
        .map_err(|e| tracing::warn!(error = %e, "local.json: разбор не удался — AI отключён"))
        .ok()
}

/// Строит RAG-подсистему из распарсенного конфига. `None` — нет embedding-секции / сервер недоступен
/// (RAG отключается, vault работает без AI). Делает реконсиляцию модели (§6.5). Эгресс — через
/// [`GuardedClient`] с единым policy/audit приложения (AC-EGR-6/13).
async fn build_rag(
    root: &Path,
    db: &Database,
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<(
    Arc<dyn EmbeddingProvider>,
    Arc<VectorIndex>,
    Arc<VectorIndex>,
    Arc<VectorIndex>,
    Arc<VectorIndex>,
    bool,
)> {
    let emb = cfg.ai.embedding.as_ref()?;
    let model = emb.model.clone().unwrap_or_else(|| "embedding".to_string());

    // Размерность: из конфига или пробным эмбеддингом у сервера (§6.5 — не хардкод).
    // Проба — Feature::Probe, короткий таймаут (30 с, как до рефактора).
    let dim = match emb.dim {
        Some(d) => d,
        None => {
            let probe = GuardedClient::for_probe(
                policy.clone(),
                audit.clone(),
                std::time::Duration::from_secs(30),
            )
            .map_err(|e| tracing::warn!(error = %e, "probe-клиент не построился — RAG отключён"))
            .ok()?;
            OpenAiEmbedder::probe_dim(&probe, &emb.url, &model)
                .await
                .map_err(
                    |e| tracing::warn!(error = %e, "проба размерности не удалась — RAG отключён"),
                )
                .ok()?
        }
    };

    let guarded = GuardedClient::for_embedding(policy.clone(), audit.clone(), emb.timeout())
        .map_err(|e| tracing::warn!(error = %e, "эмбеддер не инициализирован — RAG отключён"))
        .ok()?;
    let embedder = OpenAiEmbedder::new(
        &guarded,
        EgressFeature::Embed,
        &emb.url,
        &model,
        dim,
        ai::default_prefixes(&model),
    );

    // §6.5: смена модели/размерности инвалидирует чанки и векторы → force-переиндексация.
    let force = reconcile_embedding_model(db, root, &model, dim)
        .await
        .ok()?;

    let vectors = VectorIndex::open(root.join(".nexus").join("vectors.usearch"), dim)
        .map_err(|e| tracing::warn!(error = %e, "usearch open не удался — RAG отключён"))
        .ok()?;
    // Отдельный индекс памяти переписки (N4, RAG по чат-сессиям): тот же эмбеддер/dim, но свои
    // ключи (id сообщений) — не пересекается с чанками заметок. Параллельный канал выдачи, чтобы
    // переписка не глушила заметки в ранжировании (решение владельца + BACKLOG).
    let chat_vectors = VectorIndex::open(root.join(".nexus").join("chat_vectors.usearch"), dim)
        .map_err(|e| tracing::warn!(error = %e, "chat_vectors open не удался — память чата off"))
        .ok()?;
    // MEM: индекс памяти агента (явные факты) — свои ключи (id факта), тот же эмбеддер/dim. Параллельный
    // канал, как chat_vectors; per-vault (в .nexus этого хранилища) — память не течёт между vault'ами.
    let memory_vectors = VectorIndex::open(root.join(".nexus").join("memory_vectors.usearch"), dim)
        .map_err(
            |e| tracing::warn!(error = %e, "memory_vectors open не удался — память агента off"),
        )
        .ok()?;
    // EP: индекс эпизодической памяти (саммари сессий) — ключи = `chat_episodes.id`, тот же эмбеддер/dim.
    // Параллельный канал, как chat_vectors/memory_vectors; per-vault. Заполняется rollup-джобой/бэкфиллом.
    let episode_vectors = VectorIndex::open(
        root.join(".nexus").join("episode_vectors.usearch"),
        dim,
    )
    .map_err(|e| tracing::warn!(error = %e, "episode_vectors open не удался — память эпизодов off"))
    .ok()?;

    tracing::info!(model = %model, dim, force, "RAG включён");
    Some((
        Arc::new(embedder),
        Arc::new(vectors),
        Arc::new(chat_vectors),
        Arc::new(memory_vectors),
        Arc::new(episode_vectors),
        force,
    ))
}

/// INFER-CFG: применяет к chat-провайдеру таймауты стрима/retry из `ChatConfig`
/// (first_token/idle/retry). Температуру задаёт уже `new(..., Some(c.temperature()))`; connect-таймаут —
/// у guarded-клиента. Зеркалит `agentd::apply_chat_cfg`.
fn apply_chat_cfg(p: OpenAiChatProvider, c: &ChatConfig) -> OpenAiChatProvider {
    p.with_first_token_timeout(c.first_token_timeout())
        .with_idle_timeout(c.idle_timeout())
        .with_retry_attempts(c.retry_attempts())
}

/// Строит пару chat-провайдеров из конфига (`ai.chat`): `(обычный с reasoning, быстрый без reasoning)`.
/// `None`, если секции нет или guarded-клиент не построился. Доступность сервера здесь НЕ проверяем —
/// это выяснится при первом стриме. Оба — тот же сервер/модель; быстрый шлёт `enable_thinking=false` (R2).
async fn build_chat(
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<(Arc<dyn ChatProvider>, Arc<dyn ChatProvider>)> {
    let chat = cfg.ai.chat.as_ref()?;
    let model = chat.model.clone().unwrap_or_else(|| "chat".to_string());
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone(), chat.connect_timeout())
        .map_err(|e| tracing::warn!(error = %e, "chat-провайдер не инициализирован"))
        .ok()?;
    let normal = apply_chat_cfg(
        OpenAiChatProvider::new(
            &guarded,
            EgressFeature::Chat,
            &chat.url,
            &model,
            Some(chat.temperature()),
        ),
        chat,
    );
    let fast = apply_chat_cfg(
        OpenAiChatProvider::new(
            &guarded,
            EgressFeature::Chat,
            &chat.url,
            &model,
            Some(chat.temperature()),
        ),
        chat,
    )
    .without_reasoning();
    tracing::info!(model = %model, "chat-провайдеры включены (reasoning + fast)");
    Some((Arc::new(normal), Arc::new(fast)))
}

/// Утилитарная chat-модель из `ai.fast` (мелкая, для примитивов: inline/судья/сводка reasoning).
/// `None` — секции нет / guarded-клиент не построился → вызывающий делает fallback на gemma-fast.
/// ВСЕГДА `without_reasoning()`: примитивам CoT не нужен по определению, а на `ai.fast` может жить
/// reasoning-модель — баг 2026-06-11: gemma12 на :8084 думала ~40 с над 6-словной сводкой R1, и
/// «стрим размышлений» молчал весь ответ (с Qwen3 это занимало ~1 с). Для non-thinking шаблонов
/// лишний kwarg безвреден (jinja игнорирует неизвестную переменную).
fn build_util_chat(
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<Arc<dyn ChatProvider>> {
    let fast = cfg.ai.fast.as_ref()?;
    let model = fast.model.clone().unwrap_or_else(|| "fast".to_string());
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone(), fast.connect_timeout())
        .map_err(
            |e| tracing::warn!(error = %e, "ai.fast: провайдер не создан — fallback на gemma-fast"),
        )
        .ok()?;
    let provider = apply_chat_cfg(
        OpenAiChatProvider::new(
            &guarded,
            EgressFeature::Chat,
            &fast.url,
            &model,
            Some(fast.temperature()),
        ),
        fast,
    )
    .without_reasoning();
    tracing::info!(model = %model, url = %fast.url, "ai.fast (утилитарная модель) включена");
    Some(Arc::new(provider))
}

/// Сверяет активную модель/размерность эмбеддера с `settings`. При расхождении на НЕпервом запуске
/// чистит `chunks` (+FTS триггерами) и файл векторов, пишет новые `settings`. Возвращает `force` —
/// нужна ли принудительная переиндексация (первое включение RAG ИЛИ смена модели §6.5).
async fn reconcile_embedding_model(
    db: &Database,
    root: &Path,
    model: &str,
    dim: usize,
) -> Result<bool, ()> {
    let prev_model = get_setting(db, "embedding.model")
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile: чтение settings — RAG отключён"))?;
    let prev_dim = get_setting(db, "embedding.dim")
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile: чтение settings — RAG отключён"))?;
    if prev_model.as_deref() == Some(model) && prev_dim.as_deref() == Some(&dim.to_string()) {
        return Ok(false); // та же модель — инкрементальная индексация, без переэмбеддизации
    }
    if prev_model.is_some() {
        // Модель сменилась → старые векторы несовместимы по семантике/размерности.
        db.writer()
            .call(|c| c.execute("DELETE FROM chunks", []).map(|_| ()))
            .await
            .map_err(|e| tracing::warn!(error = %e, "reconcile: очистка chunks — RAG отключён"))?;
        let _ = std::fs::remove_file(root.join(".nexus").join("vectors.usearch"));
        // EP: эпизоды эмбеддились старой моделью → дропаем их индекс и помечаем на переэмбеддинг
        // (summary-текст остаётся, бэкфилл на открытии переэмбеддит дёшево). Иначе запрос новой моделью
        // против старых векторов → DimMismatch/семантический мусор (ложная память).
        let _ = std::fs::remove_file(root.join(".nexus").join("episode_vectors.usearch"));
        // MEM (P1-4): тот же orphan-класс — факты эмбеддились старой моделью. Дропаем индекс фактов;
        // memory-бэкфилл на открытии (фильтр по `contains`) переэмбеддит их дёшево (текст в `memory_facts`
        // остаётся). chat_vectors остаётся отдельной задачей (бэкфилл переписки есть, но сброса индекса
        // под смену модели у него пока нет — вне рамок P1-4).
        let _ = std::fs::remove_file(root.join(".nexus").join("memory_vectors.usearch"));
        db.writer()
            .call(|c| {
                c.execute("UPDATE chat_episodes SET embed_model=NULL", [])
                    .map(|_| ())
            })
            .await
            .map_err(|e| tracing::warn!(error = %e, "reconcile: сброс embed_model эпизодов"))?;
        tracing::info!(from = ?prev_model, to = %model, "смена embedding-модели → переэмбеддизация vault (§6.5)");
    }
    set_setting(db, "embedding.model", model)
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile: запись settings — RAG отключён"))?;
    set_setting(db, "embedding.dim", &dim.to_string())
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile: запись settings — RAG отключён"))?;
    Ok(true)
}

/// Читает значение из таблицы `settings` (или `None`).
async fn get_setting(db: &Database, key: &str) -> Result<Option<String>, String> {
    let key = key.to_string();
    db.reader()
        .query(move |c| {
            c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .optional()
        })
        .await
        .map_err(|e| e.to_string())
}

/// Upsert значения в таблицу `settings`.
async fn set_setting(db: &Database, key: &str, value: &str) -> Result<(), String> {
    let key = key.to_string();
    let value = value.to_string();
    db.writer()
        .call(move |c| {
            c.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![key, value],
            )
            .map(|_| ())
        })
        .await
        .map_err(|e| e.to_string())
}

/// Ручной реиндекс vault (quick action «Переиндексировать» из макета home.jsx + палитра):
/// шлёт [`crate::watcher::VaultEvent::Rescan`] в watcher-петлю — полный обход `scan_vault`
/// сериализуется с fs-событиями (без второго конкурентного сканера). Возвращается сразу
/// (скан фоновый); по завершении петля шлёт `vault:changed` — фронт перечитывает вьюхи.
#[tauri::command]
pub async fn rescan_vault(state: State<'_, AppState>) -> AppResult<()> {
    let ctx = state.vault().await?;
    let tx = ctx.index_tx.as_ref().ok_or_else(|| {
        AppError::Msg("индексация vault не запущена (watcher не стартовал)".into())
    })?;
    tx.send(crate::watcher::VaultEvent::Rescan)
        .map_err(|_| AppError::Msg("петля индексации недоступна".into()))
}

/// Ленивый листинг каталога vault (`dir_path` относительный; пустая строка = корень).
#[tauri::command]
pub async fn list_dir(state: State<'_, AppState>, dir_path: String) -> AppResult<Vec<FileEntry>> {
    // Копируем корень и отпускаем лок: ФС-обход уводим в blocking-пул.
    let root = state.vault().await?.root.clone();
    let entries = tokio::task::spawn_blocking(move || vault::list_dir(&root, Path::new(&dir_path)))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?;
    Ok(entries?)
}

/// Читает содержимое файла vault (путь относительный; анти-traversal через resolve).
#[tauri::command]
pub async fn read_file(state: State<'_, AppState>, path: String) -> AppResult<String> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path(&root, Path::new(&path))?;
    Ok(tokio::fs::read_to_string(&abs).await?)
}

/// Читает содержимое файла vault ВМЕСТЕ с хешем (`Buffer.baseHash` для детекта внешних изменений,
/// SAFE-3). `read_file` оставлен для совместимости (вызовы, которым хеш не нужен).
#[tauri::command]
pub async fn read_file_meta(state: State<'_, AppState>, path: String) -> AppResult<FileMeta> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path(&root, Path::new(&path))?;
    let content = tokio::fs::read_to_string(&abs).await?;
    let hash = vault::content_hash(content.as_bytes());
    Ok(FileMeta { content, hash })
}

/// Хеш файла на диске без чтения его содержимого во фронт (дешёвая сверка `baseHash`, SAFE-3).
/// `None`, если файла нет; traversal/абсолютный путь — ошибка (анти-traversal сохранён).
#[tauri::command]
pub async fn file_hash(state: State<'_, AppState>, path: String) -> AppResult<Option<String>> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path_for_write(&root, Path::new(&path))?;
    Ok(match tokio::fs::read(&abs).await {
        Ok(bytes) => Some(vault::content_hash(&bytes)),
        Err(_) => None, // файла нет → None (не ошибка)
    })
}

/// Пишет содержимое файла vault (целевой путь может ещё не существовать). Возвращает хеш
/// записанного контента — фронт кладёт его в `Buffer.baseHash` (эхо своего сейва не поднимает
/// guard внешнего изменения, SAFE-3). `manual` (Ctrl-S/палитра vs автосейв) управляет троттлом
/// снапшота истории (SAFE-5): ручной — всегда при изменении, авто — не чаще 1/90с.
#[tauri::command]
pub async fn write_file(
    state: State<'_, AppState>,
    path: String,
    content: String,
    manual: Option<bool>,
) -> AppResult<String> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path_for_write(&root, Path::new(&path))?;
    let hash = vault::content_hash(content.as_bytes());
    let rel = path.clone();
    let manual = manual.unwrap_or(false);
    // Атомарная запись (tmp→fsync→rename) в blocking-пуле: обрыв на середине не корраптит заметку.
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        vault::atomic_write(&abs, content.as_bytes())?;
        // Снапшот истории — BEST-EFFORT: сбой не валит сам save (заметка уже атомарно на диске).
        if let Err(e) = vault::history::snapshot(&root, &rel, &content, manual) {
            tracing::warn!(error = %e, path = %rel, "history snapshot failed");
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::Msg(e.to_string()))??;
    Ok(hash)
}

/// BOARD-1: правит ОДИН плоский frontmatter-ключ заметки (статус задачи при DnD, project/priority/due,
/// Properties-панель) — единая точка записи frontmatter. Читает файл → `parser::set_frontmatter_field`
/// (хирургическая правка, сохраняет остальной YAML/тело; serde_yaml архивирован) → атомарная запись.
/// Возвращает НОВЫЙ контент+хеш: фронт кладёт хеш в `Buffer.baseHash` (анти-эхо SAFE-3) и обновляет
/// буфер, если заметка открыта. Незакрытый frontmatter → ошибка, файл НЕ перезаписан (сохранность).
#[tauri::command]
pub async fn set_frontmatter_field(
    state: State<'_, AppState>,
    path: String,
    key: String,
    value: String,
) -> AppResult<FileMeta> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path_for_write(&root, Path::new(&path))?;
    let old = tokio::fs::read_to_string(&abs).await?;
    let new_content = crate::parser::set_frontmatter_field(&old, &value_key(&key)?, &value)
        .map_err(|e| match e {
            crate::parser::FmWriteError::Malformed => {
                AppError::Msg("frontmatter: незакрытый блок --- (откройте заметку)".into())
            }
            crate::parser::FmWriteError::Unrepresentable => AppError::Msg(
                "значение нельзя сохранить в свойство (перевод строки или краевые кавычки)".into(),
            ),
            crate::parser::FmWriteError::NonScalarTarget => AppError::Msg(
                "свойство хранит список — его нельзя перезаписать одним значением (правьте файл вручную)"
                    .into(),
            ),
        })?;
    let hash = vault::content_hash(new_content.as_bytes());
    if new_content != old {
        let rel = path.clone();
        let content_for_write = new_content.clone();
        let root_for_write = root.clone();
        let expected_old = old.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            // SAFE-3+ (закрытый буфер): перечитываем диск ПЕРЕД записью. Если внешний писатель
            // (Syncthing/Dropbox/git/другой редактор) изменил файл в окне read→write — НЕ затираем
            // его правки контентом, выведенным из устаревшего `old`; возвращаем конфликт. Для ОТКРЫТЫХ
            // буферов это ловил баннер SAFE-3, но для закрытых файлов гарда не было (потеря данных).
            let current = std::fs::read_to_string(&abs)?;
            if current != expected_old {
                return Err(AppError::Msg(
                    "файл изменён извне во время правки свойства — операция отменена (перечитайте заметку и повторите)".into(),
                ));
            }
            vault::atomic_write(&abs, content_for_write.as_bytes())?;
            // Правка статуса/свойства — намеренная → снапшот истории как ручной (SAFE-5).
            if let Err(e) = vault::history::snapshot(&root_for_write, &rel, &content_for_write, true) {
                tracing::warn!(error = %e, path = %rel, "history snapshot failed (set_frontmatter_field)");
            }
            Ok(())
        })
        .await
        .map_err(|e| AppError::Msg(e.to_string()))??;
    }
    Ok(FileMeta {
        content: new_content,
        hash,
    })
}

/// Валидирует имя frontmatter-ключа (идентификатор: буквы/цифры/`_`/`-`) — анти-инъекция в YAML.
fn value_key(key: &str) -> AppResult<String> {
    let k = key.trim();
    if k.is_empty()
        || !k
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err(AppError::Msg(format!(
            "недопустимый ключ свойства: «{key}»"
        )));
    }
    Ok(k.to_string())
}

/// Канонический путь указывает В служебный каталог (`.nexus`/`.git`) — КОМПОНЕНТНАЯ проверка после
/// канонизации. Строковый `starts_with(".nexus")` обходится через `Notes/../.nexus/nexus.db` (после
/// `canonicalize` попадает в `.nexus`, но строка начинается с «Notes») и Windows-backslash (находка
/// аудита 2026-06). `Path::starts_with` сравнивает по компонентам — кросс-платформенно безопасен.
fn points_into_reserved(root: &Path, abs: &Path) -> bool {
    abs.starts_with(root.join(".nexus")) || abs.starts_with(root.join(".git"))
}

/// Валидирует относительный путь заметки ПЕРЕД построением `.nexus/history/<rel>`: иначе `rel` вида
/// `../../etc` увёл бы чтение/листинг истории за пределы vault (path-traversal, находка аудита).
/// Канонизируем РОДИТЕЛЯ (заметка могла быть удалена — история живёт отдельно от файла) и запрещаем
/// служебные пути. Зеркалит гард delete/rename.
fn validate_history_path(root: &Path, rel: &str) -> AppResult<()> {
    if rel.trim().is_empty() {
        return Err(AppError::Msg("пустой путь".into()));
    }
    let abs = vault::resolve_vault_path_for_write(root, Path::new(rel))?;
    if points_into_reserved(root, &abs) {
        return Err(AppError::Msg("недопустимый путь истории".into()));
    }
    Ok(())
}

/// Удаляет заметку/каталог в vault-локальную корзину `.nexus/.trash/` (CURATE-1) — обратимо.
/// Снимает с индекса каждый перенесённый `.md` явным `VaultEvent::Deleted` (вотчер может не
/// разложить rename каталога в игнор-папку на пофайловые события). Служебные пути запрещены.
#[tauri::command]
pub async fn delete_path(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let ctx = state.vault().await?;
    let root = ctx.root.clone();
    if path.trim().is_empty() {
        return Err(AppError::Msg("пустой путь".into()));
    }
    let abs = vault::resolve_vault_path(&root, Path::new(&path))?;
    if points_into_reserved(&root, &abs) {
        return Err(AppError::Msg("нельзя удалить служебный путь".into()));
    }
    // Собираем rel удаляемых .md ДО переноса (после переноса каталога их уже не пройти).
    let (root_c, abs_c) = (root.clone(), abs.clone());
    let rels = tokio::task::spawn_blocking(move || vault::collect_md_rels(&root_c, &abs_c))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?;
    // Перенос в корзину (atomic rename, содержимое цело).
    let (root_m, abs_m) = (root.clone(), abs.clone());
    tokio::task::spawn_blocking(move || vault::move_to_trash(&root_m, &abs_m))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))??;
    // Детерминированное снятие с индекса (remove_file идемпотентен — двойной Deleted безопасен).
    if let Some(tx) = ctx.index_tx.as_ref() {
        for rel in &rels {
            let _ = tx.send(crate::watcher::VaultEvent::Deleted(root.join(rel)));
        }
    }
    // BOARD-3 delete-хук: точечно убираем удалённые пути из board JSON order (БЕЗОПАСНЫЙ self-heal —
    // по реальному удалению, не по «отсутствию в выборке», см. ревью F1). Best-effort.
    let (root_b, rels_b) = (root.clone(), rels.clone());
    if let Err(e) = tokio::task::spawn_blocking(move || {
        crate::board::config::remove_from_orders(&root_b, &rels_b)
    })
    .await
    {
        tracing::warn!(error = %e, "board order delete-хук не выполнился");
    }
    Ok(())
}

/// Переименовывает/перемещает заметку или каталог `from`→`to` (CURATE-2). Для файла — один
/// `Renamed` (индексатор сохраняет file_id/беклинки, V2.2); для каталога — `Renamed` по каждому
/// вложенному `.md` со свопом префикса пути. Анти-overwrite: занятая цель → ошибка. Текст ссылок
/// `[[Old]]` у источников НЕ правится (беклинки целы по id; переписывание текста — CURATE-3).
#[tauri::command]
pub async fn rename_path(state: State<'_, AppState>, from: String, to: String) -> AppResult<()> {
    let ctx = state.vault().await?;
    let root = ctx.root.clone();
    if from.trim().is_empty() || to.trim().is_empty() {
        return Err(AppError::Msg("пустой путь".into()));
    }
    if from == to {
        return Ok(());
    }
    let from_abs = vault::resolve_vault_path(&root, Path::new(&from))?;
    let to_abs = vault::resolve_vault_path_for_write(&root, Path::new(&to))?;
    // Компонентная проверка ПОСЛЕ канонизации (строковый starts_with обходится через `..`/backslash).
    if points_into_reserved(&root, &from_abs) || points_into_reserved(&root, &to_abs) {
        return Err(AppError::Msg("нельзя трогать служебный путь".into()));
    }
    if to_abs.exists() {
        return Err(AppError::Msg("цель уже существует".into()));
    }
    // Карта переименований .md (rel-from → rel-to) — собрать ДО переноса.
    let is_dir = from_abs.is_dir();
    let (root_c, abs_c, from_c, to_c) = (root.clone(), from_abs.clone(), from.clone(), to.clone());
    let pairs: Vec<(String, String)> = tokio::task::spawn_blocking(move || {
        if is_dir {
            vault::collect_md_rels(&root_c, &abs_c)
                .into_iter()
                .map(|rel| {
                    let suffix = &rel[from_c.len()..]; // ведущий '/'
                    let new_rel = format!("{to_c}{suffix}");
                    (rel, new_rel)
                })
                .collect()
        } else {
            vec![(from_c, to_c)]
        }
    })
    .await
    .map_err(|e| AppError::Msg(e.to_string()))?;
    // Перенос (atomic rename файла/каталога в пределах vault).
    let (from_m, to_m) = (from_abs.clone(), to_abs.clone());
    tokio::task::spawn_blocking(move || std::fs::rename(&from_m, &to_m))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?
        .map_err(AppError::Io)?;
    // Перенос каталога истории версий `.nexus/history/<rel>` (иначе rename ломает SAFE-5/6 — история
    // становится недоступной по новому пути; находка аудита 2026-06). Best-effort: история вторична,
    // её сбой не валит rename. Один rename поддерева покрывает и файл, и каталог.
    let (root_h, from_h, to_h) = (root.clone(), from.clone(), to.clone());
    if let Err(e) =
        tokio::task::spawn_blocking(move || vault::history::move_history(&root_h, &from_h, &to_h))
            .await
            .map_err(|e| AppError::Msg(e.to_string()))?
    {
        tracing::warn!(error = %e, %from, %to, "перенос истории версий при rename не удался");
    }
    // Перенос записей индекса (file_id/беклинки сохраняются — indexer::rename_file).
    if let Some(tx) = ctx.index_tx.as_ref() {
        for (rel_from, rel_to) in &pairs {
            let _ = tx.send(crate::watcher::VaultEvent::Renamed {
                from: root.join(rel_from),
                to: root.join(rel_to),
            });
        }
    }
    // BOARD-3 rename-хук: путь карточки в board JSON патчится from→to, ПОЗИЦИЯ в колонке сохраняется
    // (§14.6). Best-effort: доски вторичны, сбой не валит rename.
    let (root_b, pairs_b) = (root.clone(), pairs.clone());
    if let Err(e) = tokio::task::spawn_blocking(move || {
        crate::board::config::rename_in_orders(&root_b, &pairs_b)
    })
    .await
    {
        tracing::warn!(error = %e, "board order rename-хук не выполнился");
    }
    Ok(())
}

/// Список версий-снапшотов заметки (SAFE-5/6): время + размер, новейший первым. Путь относительный.
#[tauri::command]
pub async fn list_versions(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<Vec<vault::history::SnapshotMeta>> {
    let root = current_root(&state).await?;
    validate_history_path(&root, &path)?; // path-traversal в .nexus/history/<rel> (находка аудита)
    Ok(
        tokio::task::spawn_blocking(move || vault::history::list_snapshots(&root, &path))
            .await
            .map_err(|e| AppError::Msg(e.to_string()))??,
    )
}

/// Содержимое версии-снапшота заметки по его `ts` (SAFE-5/6: diff/восстановление).
#[tauri::command]
pub async fn read_version(state: State<'_, AppState>, path: String, ts: u64) -> AppResult<String> {
    let root = current_root(&state).await?;
    validate_history_path(&root, &path)?; // path-traversal в .nexus/history/<rel> (находка аудита)
    Ok(
        tokio::task::spawn_blocking(move || vault::history::read_snapshot(&root, &path, ts))
            .await
            .map_err(|e| AppError::Msg(e.to_string()))??,
    )
}

/// Заметки vault (path + title) для автокомплита `[[wikilink]]`. Кросс-план #22: вместо
/// безлимитного SELECT всего vault — подстрочный `query`-фильтр + `limit` (IPC-нагрузка ограничена
/// топ-N, а не ~MB на 50k файлов). Оба параметра опциональны: без них — полный список (прежняя
/// семантика, нужна мокам/мелким vault). Фильтр — в Rust (unicode-нечувствительность к регистру,
/// которой нет у SQLite `LIKE` для кириллицы); префикс-совпадения ранжируются выше подстрочных.
#[tauri::command]
pub async fn list_notes(
    state: State<'_, AppState>,
    query: Option<String>,
    limit: Option<u32>,
) -> AppResult<Vec<NoteRef>> {
    let reader = state.vault().await?.db.reader().clone();
    let q = query.unwrap_or_default().trim().to_lowercase();
    let limit = limit.map(|l| l as usize);
    Ok(reader
        .query(move |c| {
            let mut stmt =
                c.prepare("SELECT path, title FROM files WHERE is_deleted=0 ORDER BY path")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(NoteRef {
                        path: r.get(0)?,
                        title: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(filter_rank_notes(rows, &q, limit))
        })
        .await?)
}

/// Фильтр+ранжирование заметок для автокомплита (чистая — тестируется без State). Совпадение —
/// подстрока в пути/заголовке (lowercase, unicode); префикс basename-без-`.md` или заголовка
/// ранжируется выше; внутри ранга — порядок по пути (стабильная сортировка). Пустой `q` — все.
fn filter_rank_notes(rows: Vec<NoteRef>, q: &str, limit: Option<usize>) -> Vec<NoteRef> {
    let mut ranked: Vec<(u8, NoteRef)> = rows
        .into_iter()
        .filter_map(|n| {
            let path_lc = n.path.to_lowercase();
            let title_lc = n.title.as_deref().unwrap_or_default().to_lowercase();
            if !q.is_empty() && !path_lc.contains(q) && !title_lc.contains(q) {
                return None;
            }
            let base = path_lc
                .rsplit('/')
                .next()
                .unwrap_or(&path_lc)
                .trim_end_matches(".md")
                .to_string();
            let rank = u8::from(!(q.is_empty() || base.starts_with(q) || title_lc.starts_with(q)));
            Some((rank, n))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    let mut out: Vec<NoteRef> = ranked.into_iter().map(|(_, n)| n).collect();
    if let Some(l) = limit {
        out.truncate(l);
    }
    out
}

/// Резолвит цель `[[wikilink]]` в путь файла — ТОЙ ЖЕ функцией, что индексатор резолвит links
/// (путь / +`.md` / basename, затем алиас V4.1) — кросс-план #22: фронт больше не держит полный
/// список заметок ради клика по ссылке, а алиасные ссылки начинают резолвиться и по клику.
#[tauri::command]
pub async fn resolve_note(state: State<'_, AppState>, target: String) -> AppResult<Option<String>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(reader
        .query(move |c| {
            let Some(id) = crate::indexer::resolve_target(c, &target)? else {
                return Ok(None);
            };
            c.query_row("SELECT path FROM files WHERE id = ?1", [id], |r| r.get(0))
                .optional()
        })
        .await?)
}

/// Теги vault с количеством заметок — панель «Теги» сайдбара (DP-2, макет `sidebar.jsx`).
#[tauri::command]
pub async fn list_tags(state: State<'_, AppState>) -> AppResult<Vec<crate::tags::TagCount>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(crate::tags::list_tags(&reader).await?)
}

/// Заметки с ТОЧНЫМ тегом — клик по тегу в сайдбаре (exact-фильтр вместо зашумлённого substring-поиска).
#[tauri::command]
pub async fn notes_by_tag(
    state: State<'_, AppState>,
    tag: String,
) -> AppResult<Vec<crate::vault::NoteRef>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(crate::tags::notes_by_tag(&reader, &tag).await?)
}

/// Число живых заметок индекса — статусбар «Проиндексировано · N» (DP-14, макет app.jsx).
#[tauri::command]
pub async fn notes_count(state: State<'_, AppState>) -> AppResult<i64> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(reader
        .query(|c| {
            c.query_row("SELECT COUNT(*) FROM files WHERE is_deleted = 0", [], |r| {
                r.get(0)
            })
        })
        .await?)
}

/// Unix-mtime файла vault (сек) — clock-чип doc-meta превью (DP-15, макет editor.jsx).
#[tauri::command]
pub async fn file_mtime(state: State<'_, AppState>, path: String) -> AppResult<i64> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path(&root, Path::new(&path))?;
    let meta = tokio::fs::metadata(&abs).await?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(mtime)
}

/// Корень текущего открытого vault (или [`AppError::NoVault`], если не открыт).
async fn current_root(state: &State<'_, AppState>) -> AppResult<PathBuf> {
    Ok(state.vault().await?.root.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Компонентная проверка служебных путей ловит `.nexus`/`.git` (вкл. форму после канонизации
    /// `..`), но не задевает похожие имена (`.nexusish`) — находка аудита 2026-06.
    #[test]
    fn points_into_reserved_catches_service_dirs() {
        let root = Path::new("/vault");
        assert!(points_into_reserved(
            root,
            Path::new("/vault/.nexus/nexus.db")
        ));
        assert!(points_into_reserved(root, Path::new("/vault/.nexus")));
        assert!(points_into_reserved(root, Path::new("/vault/.git/config")));
        assert!(!points_into_reserved(root, Path::new("/vault/Notes/a.md")));
        assert!(!points_into_reserved(
            root,
            Path::new("/vault/.nexusish/a.md")
        ));
    }

    /// Аудит 2026-06: validate_history_path (гард list/read_version) принимает обычную заметку, но
    /// отклоняет traversal (`..`) и служебные пути (`.nexus`) — иначе чтение `.nexus/history/<rel>`
    /// ушло бы за пределы vault.
    #[test]
    fn validate_history_path_rejects_traversal_and_reserved() {
        let dir = TempDir::new().unwrap();
        // Канонизируем root (в проде `current_root` уже канонизирован; на macOS TempDir = /var → симлинк
        // на /private/var, иначе starts_with в resolve_vault_path_for_write ложно бы не сматчился).
        let root = dir.path().canonicalize().unwrap();
        let root = root.as_path();
        std::fs::create_dir_all(root.join("Notes")).unwrap();
        std::fs::create_dir_all(root.join(".nexus")).unwrap();
        assert!(validate_history_path(root, "Notes/A.md").is_ok()); // обычная (файл может не существовать)
        assert!(validate_history_path(root, "../../../etc/passwd").is_err()); // traversal
        assert!(validate_history_path(root, ".nexus/nexus.db").is_err()); // служебный
        assert!(validate_history_path(root, "   ").is_err()); // пустой
    }

    async fn open_db(root: &Path) -> Database {
        Database::open(root.join(".nexus/nexus.db")).await.unwrap()
    }

    /// AC-EGR-13 (composition-root): `build_chat`/`build_util_chat` строят провайдеров от ОДНОГО
    /// policy через guarded-клиент — переключение политики мгновенно видно ВСЕМ провайдерам
    /// (никаких собственных клиентов мимо chokepoint).
    #[tokio::test]
    async fn build_chat_providers_share_one_policy() {
        use std::sync::atomic::AtomicBool;

        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        let audit = Arc::new(EgressAudit::default());
        let cfg = LocalConfig::parse(
            r#"{"ai":{
                "chat": { "url": "http://127.0.0.1:9", "model": "m" },
                "fast": { "url": "http://127.0.0.1:9", "model": "f" }
            }}"#,
        )
        .unwrap();
        let (chat, chat_fast) = build_chat(&cfg, &policy, &audit)
            .await
            .expect("chat построен");
        let util = build_util_chat(&cfg, &policy, &audit).expect("util построен");

        // Выключаем Chat-фичу на ЕДИНОМ policy → все три провайдера отрезаны типизированно.
        policy.set_feature_enabled(EgressFeature::Chat, false);
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let msgs = vec![crate::ai::ChatMessage::user("x")];
        for (name, p) in [
            ("chat", &chat),
            ("chat_fast", &chat_fast),
            ("chat_util", &util),
        ] {
            let res = p.stream_chat(&msgs, &mut |_| {}, &cancel).await;
            assert!(
                matches!(res, Err(crate::ai::AiError::Denied(_))),
                "{name}: провайдер обязан ходить через общий policy (AC-EGR-13): {res:?}"
            );
        }
        assert_eq!(audit.len(), 3, "каждый отказ — в общем audit (AC-EGR-4)");
    }

    async fn count_chunks(db: &Database) -> i64 {
        db.reader()
            .query(|c| c.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0)))
            .await
            .unwrap()
    }

    /// #22: фильтр+ранжирование автокомплита — подстрока по пути/заголовку (unicode lowercase),
    /// префикс basename/заголовка выше подстрочного совпадения, limit режет ПОСЛЕ ранжирования.
    #[test]
    fn filter_rank_notes_prefix_first_with_limit() {
        let note = |p: &str, t: Option<&str>| NoteRef {
            path: p.to_string(),
            title: t.map(str::to_string),
        };
        let rows = vec![
            note("Notes/CrossRoad.md", None), // подстрочное совпадение basename
            note("Plans.md", Some("Roadmap-цели")), // префикс заголовка
            note("Projects/Roadmap.md", Some("План")), // префикс basename
            note("Прочее.md", None),          // не совпадает
        ];
        let out = filter_rank_notes(rows.clone(), "road", None);
        assert_eq!(
            out.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(),
            vec!["Plans.md", "Projects/Roadmap.md", "Notes/CrossRoad.md"],
            "префикс-совпадения первыми (внутри ранга — порядок по пути)"
        );
        // Кириллица: lowercase-подстрока работает (SQLite LIKE так не умеет).
        let cyr = filter_rank_notes(rows.clone(), "проч", None);
        assert_eq!(cyr.len(), 1);
        assert_eq!(cyr[0].path, "Прочее.md");
        // limit режет после ранжирования: остаётся лучший (префиксный) матч.
        let top1 = filter_rank_notes(rows.clone(), "road", Some(1));
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].path, "Plans.md");
        // Пустой запрос — все в порядке пути, limit применяется.
        assert_eq!(filter_rank_notes(rows, "", Some(2)).len(), 2);
    }

    /// #22: `resolve_note`-резолв кликом = резолв индексатора (одна функция): путь / +.md /
    /// basename, затем алиас (V4.1) — алиасные ссылки резолвятся и по клику.
    #[tokio::test]
    async fn resolve_note_matches_indexer_semantics_including_aliases() {
        let dir = TempDir::new().unwrap();
        let db = open_db(dir.path()).await;
        db.writer()
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO files(path,hash,title,created_at,updated_at,indexed_at,size_bytes) \
                     VALUES ('Notes/Кошка.md','h1','О кошках',0,0,0,1), \
                            ('Inbox.md','h2',NULL,0,0,0,1); \
                     INSERT INTO aliases(file_id,alias) \
                     SELECT id,'Мурка' FROM files WHERE path='Notes/Кошка.md';",
                )
                .map(|_| ())
            })
            .await
            .unwrap();
        let resolve = |target: &'static str| {
            let reader = db.reader().clone();
            async move {
                reader
                    .query(move |c| {
                        let Some(id) = crate::indexer::resolve_target(c, target)? else {
                            return Ok(None);
                        };
                        c.query_row("SELECT path FROM files WHERE id=?1", [id], |r| {
                            r.get::<_, String>(0)
                        })
                        .optional()
                    })
                    .await
                    .unwrap()
            }
        };
        assert_eq!(
            resolve("Кошка").await.as_deref(),
            Some("Notes/Кошка.md"),
            "basename"
        );
        assert_eq!(
            resolve("Notes/Кошка.md").await.as_deref(),
            Some("Notes/Кошка.md")
        );
        assert_eq!(resolve("Inbox").await.as_deref(), Some("Inbox.md"), "+.md");
        assert_eq!(
            resolve("Мурка").await.as_deref(),
            Some("Notes/Кошка.md"),
            "алиас V4.1"
        );
        assert_eq!(resolve("Нету такой").await, None);
    }

    /// §6.5: первое включение RAG пишет settings и требует force; та же модель — без force.
    #[tokio::test]
    async fn reconcile_first_run_sets_settings_and_forces() {
        let dir = TempDir::new().unwrap();
        let db = open_db(dir.path()).await;

        let force = reconcile_embedding_model(&db, dir.path(), "nomic", 768)
            .await
            .unwrap();
        assert!(force, "первое включение RAG → force-переиндексация");
        assert_eq!(
            get_setting(&db, "embedding.model")
                .await
                .unwrap()
                .as_deref(),
            Some("nomic")
        );
        assert_eq!(
            get_setting(&db, "embedding.dim").await.unwrap().as_deref(),
            Some("768")
        );

        let again = reconcile_embedding_model(&db, dir.path(), "nomic", 768)
            .await
            .unwrap();
        assert!(!again, "та же модель/dim → без переэмбеддизации");
    }

    /// §6.5 (AC-Б5-2): смена модели чистит chunks (+FTS триггерами) и требует force.
    #[tokio::test]
    async fn reconcile_model_change_wipes_chunks_and_forces() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let db = open_db(root).await;
        reconcile_embedding_model(&db, root, "nomic", 768)
            .await
            .unwrap();

        // Файл + чанк (как после индексации).
        db.writer()
            .call(|c| {
                c.execute(
                    "INSERT INTO files (path,hash,created_at,updated_at,indexed_at,size_bytes) \
                     VALUES ('A.md','h',0,0,0,1)",
                    [],
                )?;
                let fid: i64 =
                    c.query_row("SELECT id FROM files WHERE path='A.md'", [], |r| r.get(0))?;
                c.execute(
                    "INSERT INTO chunks (file_id,chunk_index,content,char_start,char_end,token_count) \
                     VALUES (?1,0,'text',0,4,1)",
                    [fid],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(count_chunks(&db).await, 1);

        let force = reconcile_embedding_model(&db, root, "bge-m3", 1024)
            .await
            .unwrap();
        assert!(force, "смена модели → force");
        assert_eq!(
            count_chunks(&db).await,
            0,
            "смена модели очистила chunks (§6.5)"
        );
        assert_eq!(
            get_setting(&db, "embedding.dim").await.unwrap().as_deref(),
            Some("1024")
        );
    }

    // ── R-3b: ХАРАКТЕРИЗАЦИЯ сборки провайдеров open_vault (REFACTOR-PLAN §3, thermo-смелл №3) ─────
    //
    // Фикстура «до»: снимки ВСЕХ конфиг-наблюдаемых параметров провайдеров (`debug_params`) сняты со
    // СТАРЫХ desktop-строителей (`build_chat`/`build_util_chat` + embedder-часть `build_rag` +
    // композиция `open_vault`) в КОММИТЕ 1 этого среза (двухкоммитный приём R-2/R-3a) — и НЕ меняются
    // при переключении сборки на канон `nexus_core::bootstrap::ProviderSet` (коммит 2).
    // Строки-снимки НЕ «пере-снимать» при рефакторе — они и есть контракт.

    /// «Полный» конфиг: chat+fast+embedding, модели заданы, dim задан (без сетевой пробы),
    /// таймауты/температуры дефолтные.
    const BOOT_CFG_FULL: &str = r#"{
      "ai": {
        "chat":      { "url": "http://192.168.0.28:8080", "model": "qwen3-30b", "context_window": 32768 },
        "fast":      { "url": "http://192.168.0.28:8084", "model": "gemma-4b" },
        "embedding": { "url": "http://192.168.0.28:8083", "model": "bge-m3", "dim": 1024 }
      }
    }"#;

    /// Без `ai.fast`: chat_util обязан упасть в fallback на chat_fast (композиция `open_vault`);
    /// embedding — nomic (характеризует task-префиксы).
    const BOOT_CFG_NO_FAST: &str = r#"{
      "ai": {
        "chat":      { "url": "http://127.0.0.1:9101", "model": "qwen3-30b" },
        "embedding": { "url": "http://127.0.0.1:9103", "model": "nomic-embed-text", "dim": 768 }
      }
    }"#;

    /// Без `ai.embedding`: RAG off, chat+fast живут.
    const BOOT_CFG_NO_EMBEDDING: &str = r#"{
      "ai": {
        "chat": { "url": "http://127.0.0.1:9101", "model": "qwen3-30b" },
        "fast": { "url": "http://127.0.0.1:9104", "model": "gemma-4b" }
      }
    }"#;

    /// Кастомные таймауты/температуры/ретраи ВЕЗДЕ; модели НЕ заданы (дефолты "chat"/"fast"/
    /// "embedding"); chat-url с хвостом `/v1` (характеризует нормализацию `api_base`).
    const BOOT_CFG_CUSTOM: &str = r#"{
      "ai": {
        "chat": {
          "url": "http://127.0.0.1:9201/v1",
          "connect_timeout_secs": 5,
          "first_token_timeout_secs": 45,
          "idle_timeout_secs": 10,
          "retry_attempts": 7,
          "temperature": 0.9
        },
        "fast": {
          "url": "http://127.0.0.1:9202",
          "connect_timeout_secs": 2,
          "first_token_timeout_secs": 20,
          "idle_timeout_secs": 4,
          "retry_attempts": 1,
          "temperature": 0.05
        },
        "embedding": { "url": "http://127.0.0.1:9203", "dim": 512, "timeout_secs": 120 }
      }
    }"#;

    /// Пустой конфиг: ни одного провайдера.
    const BOOT_CFG_EMPTY: &str = r#"{}"#;

    fn boot_cfg(json: &str) -> LocalConfig {
        LocalConfig::parse(json).expect("фикстурный конфиг валиден")
    }

    fn boot_edges() -> (Arc<EgressPolicy>, Arc<EgressAudit>) {
        use std::sync::atomic::AtomicBool;
        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        (policy, Arc::new(EgressAudit::default()))
    }

    /// Chat-каналы ТЕКУЩИМ путём desktop — РЕПЛИКА композиции `open_vault` на старых строителях
    /// (`build_chat` → `build_util_chat` + fallback на chat_fast); коммит 1 характеризации —
    /// в коммите 2 переключается на канон `bootstrap::ProviderSet` (опции desktop), ассерты
    /// НЕ меняются.
    struct BootProviders {
        chat: Option<Arc<dyn ChatProvider>>,
        chat_fast: Option<Arc<dyn ChatProvider>>,
        chat_util: Option<Arc<dyn ChatProvider>>,
    }

    async fn build_current_way(cfg_json: &str) -> BootProviders {
        let cfg = boot_cfg(cfg_json);
        let (policy, audit) = boot_edges();
        let (chat, chat_fast) = match build_chat(&cfg, &policy, &audit).await {
            Some((normal, fast)) => (Some(normal), Some(fast)),
            None => (None, None),
        };
        let chat_util = build_util_chat(&cfg, &policy, &audit).or_else(|| chat_fast.clone());
        BootProviders {
            chat,
            chat_fast,
            chat_util,
        }
    }

    /// Эмбеддер ТЕКУЩИМ путём desktop: СКВОЗЬ `build_rag` (reconcile+usearch на временном vault —
    /// живой RAG-путь `open_vault`; dim задан в фикстурах → сетевой пробы нет).
    async fn build_embedder_current_way(cfg_json: &str) -> Option<Arc<dyn EmbeddingProvider>> {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let db = open_db(&root).await;
        let cfg = boot_cfg(cfg_json);
        let (policy, audit) = boot_edges();
        build_rag(&root, &db, &cfg, &policy, &audit)
            .await
            .map(|r| r.0)
    }

    /// Полный конфиг: пара chat-провайдеров — один сервер/модель/температура/таймауты/ретрай,
    /// различие ТОЛЬКО в reasoning (normal ON, fast OFF).
    #[tokio::test]
    async fn boot_chat_pair_full_config() {
        let p = build_current_way(BOOT_CFG_FULL).await;
        assert_eq!(
            p.chat.expect("ai.chat → провайдер").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8080/v1/chat/completions", model: "qwen3-30b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: true }"#
        );
        assert_eq!(
            p.chat_fast.expect("ai.chat → быстрый").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8080/v1/chat/completions", model: "qwen3-30b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: false }"#
        );
    }

    /// Полный конфиг: утилитарная модель из `ai.fast` — свой сервер/модель, ВСЕГДА без reasoning.
    #[tokio::test]
    async fn boot_util_chat_full_config() {
        let p = build_current_way(BOOT_CFG_FULL).await;
        assert_eq!(
            p.chat_util.expect("ai.fast → утилитарная").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=30s)", feature: Chat, endpoint: "http://192.168.0.28:8084/v1/chat/completions", model: "gemma-4b", temperature: 0.3, first_token_timeout: 300s, idle_timeout: 90s, retry: RetryPolicy { max_attempts: 3, base: 300ms, cap: 2s }, enable_thinking: false }"#
        );
    }

    /// Кастомные таймауты: ВСЕ INFER-CFG параметры конфига доезжают до провайдеров (connect/
    /// first_token/idle/retry/temperature), дефолт-модели "chat"/"fast", `/v1`-хвост не удваивается.
    #[tokio::test]
    async fn boot_custom_timeouts_reach_providers() {
        let p = build_current_way(BOOT_CFG_CUSTOM).await;
        assert_eq!(
            p.chat.expect("ai.chat → провайдер").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=5s)", feature: Chat, endpoint: "http://127.0.0.1:9201/v1/chat/completions", model: "chat", temperature: 0.9, first_token_timeout: 45s, idle_timeout: 10s, retry: RetryPolicy { max_attempts: 7, base: 300ms, cap: 2s }, enable_thinking: true }"#
        );
        assert_eq!(
            p.chat_util.expect("ai.fast → утилитарная").debug_params(),
            r#"OpenAiChatProvider { client: "for_chat(connect_timeout=2s)", feature: Chat, endpoint: "http://127.0.0.1:9202/v1/chat/completions", model: "fast", temperature: 0.05, first_token_timeout: 20s, idle_timeout: 4s, retry: RetryPolicy { max_attempts: 1, base: 300ms, cap: 2s }, enable_thinking: false }"#
        );
    }

    /// Без `ai.fast`: утилитарный канал = ТОТ ЖЕ Arc, что chat_fast (fallback композиции — дайджест/
    /// примитивы не дохнут без отдельной мелкой модели).
    #[tokio::test]
    async fn boot_util_falls_back_to_chat_fast() {
        let p = build_current_way(BOOT_CFG_NO_FAST).await;
        let fast = p.chat_fast.expect("ai.chat → быстрый");
        let util = p.chat_util.expect("fallback → chat_fast");
        assert!(
            Arc::ptr_eq(&fast, &util),
            "без ai.fast chat_util обязан быть ТЕМ ЖЕ провайдером, что chat_fast"
        );
    }

    /// Пустой конфиг: ни одного chat-канала (vault работает без AI — local-first).
    #[tokio::test]
    async fn boot_empty_config_builds_nothing() {
        let p = build_current_way(BOOT_CFG_EMPTY).await;
        assert!(p.chat.is_none(), "нет ai.chat → нет chat");
        assert!(p.chat_fast.is_none(), "нет ai.chat → нет chat_fast");
        assert!(p.chat_util.is_none(), "нет ai.fast И нет chat_fast → None");
    }

    /// Полный конфиг: эмбеддер — url/model/dim/таймаут guarded-клиента; bge → БЕЗ task-префиксов.
    #[tokio::test]
    async fn boot_embedder_full_config() {
        let e = build_embedder_current_way(BOOT_CFG_FULL)
            .await
            .expect("ai.embedding+dim → эмбеддер без пробы");
        assert_eq!(
            e.debug_params(),
            r#"OpenAiEmbedder { client: "for_embedding(timeout=60s)", feature: Embed, endpoint: "http://192.168.0.28:8083/v1/embeddings", model: "bge-m3", dim: 1024, query_prefix: "", document_prefix: "" }"#
        );
    }

    /// nomic-модель: task-префиксы `search_query:`/`search_document:` применены (default_prefixes).
    #[tokio::test]
    async fn boot_embedder_nomic_prefixes() {
        let e = build_embedder_current_way(BOOT_CFG_NO_FAST)
            .await
            .expect("ai.embedding+dim → эмбеддер");
        assert_eq!(
            e.debug_params(),
            r#"OpenAiEmbedder { client: "for_embedding(timeout=60s)", feature: Embed, endpoint: "http://127.0.0.1:9103/v1/embeddings", model: "nomic-embed-text", dim: 768, query_prefix: "search_query: ", document_prefix: "search_document: " }"#
        );
    }

    /// Кастомный embedding-таймаут + дефолт-модель "embedding" доезжают до эмбеддера.
    #[tokio::test]
    async fn boot_embedder_custom_timeout() {
        let e = build_embedder_current_way(BOOT_CFG_CUSTOM)
            .await
            .expect("ai.embedding+dim → эмбеддер");
        assert_eq!(
            e.debug_params(),
            r#"OpenAiEmbedder { client: "for_embedding(timeout=120s)", feature: Embed, endpoint: "http://127.0.0.1:9203/v1/embeddings", model: "embedding", dim: 512, query_prefix: "", document_prefix: "" }"#
        );
    }

    /// Без `ai.embedding` эмбеддера нет (RAG off) — chat-провайдеры при этом живут (см. фикстуру).
    #[tokio::test]
    async fn boot_no_embedding_no_embedder() {
        assert!(
            build_embedder_current_way(BOOT_CFG_NO_EMBEDDING)
                .await
                .is_none(),
            "нет ai.embedding → нет эмбеддера"
        );
    }
}
