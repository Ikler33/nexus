//! Команды vault: открытие хранилища и ленивый листинг каталогов.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::OptionalExtension;
use tauri::State;

use crate::ai::{
    self, AIClient, ChatProvider, EmbeddingProvider, LocalConfig, OpenAiChatProvider,
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

    let info = VaultInfo {
        root: root.to_string_lossy().into_owned(),
        name: vault::vault_name(&root),
    };

    // Конфиг `.nexus/local.json` парсим ОДИН раз (раньше — дважды: build_rag + build_chat), кросс-план #8.
    let local_cfg = load_local_config(&root).await;

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
    let (vectors, chat_vectors, embedder, indexer) = match rag {
        Some((embedder, vec_index, chat_vec_index, force)) => {
            let idx = crate::indexer::Indexer::with_rag(
                &db,
                root.clone(),
                embedder.clone(),
                vec_index.clone(),
                force,
            );
            (Some(vec_index), Some(chat_vec_index), Some(embedder), idx)
        }
        None => (
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
    let (watcher, index_tx) = match crate::indexer::spawn(indexer, app.clone()) {
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
    let news_chat = chat_util.clone().or_else(|| chat_fast.clone());
    let news_active = if let (Some(config_path), Some(news_chat)) = (&news_config_path, news_chat) {
        let handler: Arc<dyn crate::scheduler::JobHandler> =
            Arc::new(crate::news::NewsFeedHandler {
                fetcher: Arc::new(crate::news::GuardedNewsFetcher::new(
                    state.egress_policy.clone(),
                    state.egress_audit.clone(),
                    Arc::new(crate::news::SystemResolver),
                )),
                chat: news_chat,
                writer: db.writer().clone(),
                reader: db.reader().clone(),
                config_path: config_path.clone(),
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
    if chat.is_some() {
        recurring.insert(crate::digest::KIND_DIGEST.to_string(), DAY_SECS);
    }
    if chat.is_some() && vectors.is_some() {
        recurring.insert(crate::contradictions::KIND_CONTRA.to_string(), DAY_SECS);
    }
    // On-change (slice 7): дайджест+противоречия перезапускаются после правок vault (с дебаунсом).
    let on_change: Vec<String> = recurring.keys().cloned().collect();
    // Context drift (H5) — scheduled-only (раз/сутки; концепт: «чаще нет смысла»): в `recurring`, но НЕ в
    // `on_change` — добавляем ПОСЛЕ снятия on_change, чтобы он не реагировал на каждую правку.
    if chat.is_some() {
        recurring.insert(
            crate::home::widgets::widget_kind(crate::home::insights::KEY_CONTEXT_DRIFT),
            DAY_SECS,
        );
    }
    // Open questions (H5) — AIP-5: проактивно раз/сутки (как context drift), scheduled-only (НЕ on-change,
    // добавлено после снятия on_change). Раньше — manual-only; теперь генерируется само, чтобы карточка
    // не висела пустой с «нажми обновить». Хендлер на `chat_util`, поэтому и гейт по нему.
    if chat_util.is_some() {
        recurring.insert(
            crate::home::widgets::widget_kind(crate::home::insights::KEY_OPEN_QUESTIONS),
            DAY_SECS,
        );
    }
    // Stale radar (H4) — AIP-хвост: слой 2 теперь ПРОАКТИВЕН (раз/сутки, scheduled-only, как
    // open_questions; добавлено после снятия on_change — правка делает заметку МЕНЕЕ устаревшей, спешить
    // с переобогащением незачем). Per-note кэш делает повторный прогон дешёвым (пропуск валидного).
    if chat_util.is_some() {
        recurring.insert(crate::home::stale::KIND_STALE.to_string(), DAY_SECS);
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
        if chat.is_some() {
            seeds.push(crate::home::insights::KEY_CONTEXT_DRIFT);
        }
        if chat_util.is_some() {
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
        && crate::contradictions::should_generate(db.reader())
            .await
            .unwrap_or(false)
    {
        let _ =
            crate::scheduler::enqueue(db.writer(), crate::contradictions::KIND_CONTRA, "", 0, 2)
                .await;
    }

    // Фасад §4.3 (AC-EGR-13): ВСЕ провайдеры + политика — одним полем; policy — тот же Arc, что
    // в AppState (один экземпляр на приложение, через него hot-swap пересоберёт guarded-клиент).
    *state.vault.write().await = Some(VaultContext {
        root,
        db,
        vectors,
        chat_vectors,
        ai: AIClient {
            chat,
            chat_fast,
            chat_util,
            embedder,
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
    tracing::info!(vault = %info.root, "opened vault");
    Ok(info)
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

    let guarded = GuardedClient::for_embedding(policy.clone(), audit.clone())
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

    tracing::info!(model = %model, dim, force, "RAG включён");
    Some((
        Arc::new(embedder),
        Arc::new(vectors),
        Arc::new(chat_vectors),
        force,
    ))
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
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone())
        .map_err(|e| tracing::warn!(error = %e, "chat-провайдер не инициализирован"))
        .ok()?;
    let normal = OpenAiChatProvider::new(&guarded, EgressFeature::Chat, &chat.url, &model, None);
    let fast = OpenAiChatProvider::new(&guarded, EgressFeature::Chat, &chat.url, &model, None)
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
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone())
        .map_err(
            |e| tracing::warn!(error = %e, "ai.fast: провайдер не создан — fallback на gemma-fast"),
        )
        .ok()?;
    let provider = OpenAiChatProvider::new(&guarded, EgressFeature::Chat, &fast.url, &model, None)
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

/// Канонический путь указывает В служебный каталог (`.nexus`/`.git`) — КОМПОНЕНТНАЯ проверка после
/// канонизации. Строковый `starts_with(".nexus")` обходится через `Notes/../.nexus/nexus.db` (после
/// `canonicalize` попадает в `.nexus`, но строка начинается с «Notes») и Windows-backslash (находка
/// аудита 2026-06). `Path::starts_with` сравнивает по компонентам — кросс-платформенно безопасен.
fn points_into_reserved(root: &Path, abs: &Path) -> bool {
    abs.starts_with(root.join(".nexus")) || abs.starts_with(root.join(".git"))
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
    Ok(())
}

/// Список версий-снапшотов заметки (SAFE-5/6): время + размер, новейший первым. Путь относительный.
#[tauri::command]
pub async fn list_versions(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<Vec<vault::history::SnapshotMeta>> {
    let root = current_root(&state).await?;
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
}
