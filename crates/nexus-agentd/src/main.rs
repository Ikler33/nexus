//! nexus-agentd — headless agent-service (CORE-2a, топология A).
//!
//! МИНИМАЛЬНЫЙ бинарь, доказывающий, что `nexus-core` переиспользуемо БЕЗ Tauri-десктопа: открывает
//! vault headless (БД + конфиг + AIClient + GuardedClient + индексатор-фундамент) и крутит воркер-луп
//! планировщика. НЕТ Tauri, НЕТ зависимостей `apps/desktop`, НЕТ agent-loop/tools/skills — чистый
//! SKELETON. Композиция реплицирует МИНИМУМ проводки из app-приватных `open_vault`/`build_rag`/
//! `build_chat`/`build_util_chat` (`apps/desktop/src-tauri/src/commands/vault.rs`), используя ТОЛЬКО
//! публичные типы `nexus-core` (копируем тела, app не трогаем — PREFER copy over expose).
//!
//! Запуск: `nexus-agentd <vault>` или `NEXUS_VAULT=<vault> nexus-agentd`.
//! `NEXUS_AGENTD_SMOKE=1` → один ограниченный прогон (несколько тиков) и выход 0 (headless smoke).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use nexus_core::ai::tools::{OpenAiToolProvider, ToolCapableProvider};
use nexus_core::ai::{
    self, AIClient, ChatProvider, EmbeddingProvider, LocalConfig, OpenAiChatProvider,
    OpenAiEmbedder,
};
use nexus_core::db::Database;
use nexus_core::net::{EgressAudit, EgressFeature, EgressPolicy, GuardedClient};
use nexus_core::vector::VectorIndex;

mod health;

/// Сколько тиков прокрутить в smoke-режиме перед выходом. Тик планировщика — `TICK_SECS` (5 с в ядре),
/// поэтому ждём с запасом, чтобы воркер успел стартовать (crash-recovery) и хотя бы раз тикнуть.
const SMOKE_TICKS_DEADLINE: Duration = Duration::from_secs(8);

/// TTL «застрявшего» прогона агента (AGENT-2 crash-recovery): прогон в 'running', не обновлявшийся
/// дольше этого, считается осиротевшим крахом → requeue в 'queued' на старте. Щедрый запас над самым
/// долгим легитимным прогоном (LoopBounds.wall_clock = 5 мин): 30 мин — живой прогон heartbeat'ит
/// updated_at (mark_running/bump_step/finish), поэтому в TTL не попадёт.
const AGENT_RUN_STALE_TTL_SECS: i64 = 30 * 60;

#[tokio::main]
async fn main() {
    // tracing: компактный stdout-подписчик (без Tauri-файлового лога десктопа). Уровень из
    // `RUST_LOG` (грубо: trace/debug/info/warn/error), дефолт — info, чтобы старт/тики/egress-
    // предупреждения были видны headless. EnvFilter НЕ используем намеренно: фича `env-filter`
    // тянет `regex`/`matchers` (нет в lockfile) → офлайн-скачивание; десктоп тоже на LevelFilter.
    tracing_subscriber::fmt()
        .with_max_level(log_level_from_env())
        .init();

    if let Err(e) = run().await {
        tracing::error!(error = %e, "nexus-agentd: фатальная ошибка");
        std::process::exit(1);
    }
}

/// Грубый разбор `RUST_LOG` в `LevelFilter` (без env-filter-зависимостей). Неизвестное → info.
fn log_level_from_env() -> tracing::level_filters::LevelFilter {
    use tracing::level_filters::LevelFilter;
    match std::env::var("RUST_LOG")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        "off" => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    }
}

/// Источник vault: `argv[1]` приоритетнее, иначе env `NEXUS_VAULT`. Ясная ошибка, если не задан.
fn vault_path_from_args() -> Result<PathBuf, String> {
    if let Some(arg) = std::env::args().nth(1) {
        return Ok(PathBuf::from(arg));
    }
    if let Ok(env) = std::env::var("NEXUS_VAULT") {
        if !env.is_empty() {
            return Ok(PathBuf::from(env));
        }
    }
    Err("укажите путь к vault: `nexus-agentd <vault>` или env NEXUS_VAULT".to_string())
}

/// Композиционный корень headless: повторяет минимум `open_vault` без Tauri/AppState.
async fn run() -> Result<(), String> {
    let raw = vault_path_from_args()?;
    let root = raw
        .canonicalize()
        .map_err(|e| format!("vault path {}: {e}", raw.display()))?;
    if !root.is_dir() {
        return Err(format!("vault: {} — не каталог", root.display()));
    }

    // БД в `.nexus/nexus.db` — `Database::open` сам гоняет миграции (создаёт схему при первом старте).
    let db = Database::open(root.join(".nexus").join("nexus.db"))
        .await
        .map_err(|e| format!("открытие БД: {e}"))?;

    // Egress-граница (ADR-005-ext): ОДНА политика + ОДИН audit на процесс (как `AppState`). Kill-switch
    // `offline` — собственный атомик (в десктопе шарится с UI; headless управления им пока нет).
    // ВНИМАНИЕ (CORE-2a skeleton): persisted `egress.json` (offline + per-feature opt-out; десктоп грузит
    // его в lib.rs через `net::persist::load`→apply) здесь НЕ восстанавливается — политика строится с
    // offline=false и Chat/Embed/Probe ON. Скелет реального egress не делает (smoke ai=false; единственный
    // возможный — probe_dim на СВОЙ allowlisted ai.embedding-хост). TODO ДО egress-способного agentd:
    // подгрузить egress.json и применить offline+флаги, иначе headless-agentd проигнорирует kill-switch.
    let egress_offline = Arc::new(AtomicBool::new(false));
    let egress_policy = Arc::new(EgressPolicy::new(egress_offline));
    let egress_audit = Arc::new(EgressAudit::default());
    // P0-b: durable-сток egress-audit. БД уже открыта выше → подключаем сразу (в headless нет pre-vault
    // окна, как у десктопа). Весь реальный эгресс agentd durable-аудитится write-before-act.
    egress_audit.set_writer(db.writer().clone());

    // Конфиг `.nexus/local.json` (один разбор, как кросс-план #8). Нет/битый → AI отключён (local-first).
    let local_cfg = load_local_config(&root).await;

    // Авто-allowlist (E4): хосты явных `ai.*.url`. Нет конфига → пусто (fail-closed для публичных хостов).
    egress_policy.set_allowlist(
        local_cfg
            .as_ref()
            .map(LocalConfig::egress_hosts)
            .unwrap_or_default(),
    );

    // RAG-фундамент + ПАМЯТЬ агента (AGENT-MEM-1): эмбеддер + note-RAG индекс + ТРИ индекса памяти
    // (переписка/факты/эпизоды). build_rag_min теперь ГОНИТ reconcile_embedding_model (CORE-2a #2):
    // stale on-disk индекс под другой моделью/dim сбрасывается ДО открытия → нет DimMismatch на первом
    // search/upsert (раньше skeleton наследовал чужой индекс — комментарий-предупреждение был, гард — нет).
    let rag = match &local_cfg {
        Some(cfg) => build_rag_min(&db, &root, cfg, &egress_policy, &egress_audit).await,
        None => None,
    };
    let (vectors, chat_vectors, memory_vectors, episode_vectors, embedder) = match rag {
        Some(r) => (
            Some(r.vectors),
            Some(r.chat_vectors),
            Some(r.memory_vectors),
            Some(r.episode_vectors),
            Some(r.embedder),
        ),
        None => (None, None, None, None, None),
    };

    // Chat-провайдеры (реплика build_chat): обычный (reasoning) + быстрый (без reasoning).
    let (chat, chat_fast) = match &local_cfg {
        Some(cfg) => match build_chat_min(cfg, &egress_policy, &egress_audit) {
            Some((normal, fast)) => (Some(normal), Some(fast)),
            None => (None, None),
        },
        None => (None, None),
    };
    // Утилитарная модель (реплика build_util_chat): `ai.fast` без reasoning, fallback на chat_fast.
    let chat_util = match &local_cfg {
        Some(cfg) => build_util_chat_min(cfg, &egress_policy, &egress_audit),
        None => None,
    }
    .or_else(|| chat_fast.clone());

    // AGENT-1 (I-5): tool-capable провайдер из ТОГО ЖЕ GuardedClient::for_chat + EgressFeature::Chat,
    // что и build_chat_min — но ОТДЕЛЬНЫЙ тип (OpenAiToolProvider), tools не протекают в chat-путь.
    // Это единственное место в проекте (вне ядра/тестов), где он конструируется (десктоп держит None).
    let agent_tools = match &local_cfg {
        Some(cfg) => build_agent_tools_min(cfg, &egress_policy, &egress_audit),
        None => None,
    };

    // AIClient (тот же контейнер, что десктоп кладёт в VaultContext) — собран из ядровых провайдеров.
    // AGENT-2: потребляется AgentRunHandler (нужен `agent_tools` + токенайзер/бюджет внутри хендлера),
    // поэтому в `Arc` (хендлер держит долю).
    let ai_client = Arc::new(AIClient {
        chat,
        chat_fast,
        chat_util,
        embedder: embedder.clone(),
        agent_tools,
        policy: egress_policy.clone(),
    });
    let ai_ready = ai_client.chat.is_some();
    let embed_ready = ai_client.embedder.is_some();
    let agent_ready = ai_client.agent_tools.is_some();
    // Окно контекста модели агента (для ContextBudget внутри хендлера) — из `ai.chat.context_window`.
    let agent_context_window = local_cfg
        .as_ref()
        .and_then(|c| c.ai.chat.as_ref())
        .and_then(|c| c.context_window);

    // Индексатор-фундамент. Десктоп тут спавнит watcher с Tauri-хуками (прогресс/файл-изменён); headless
    // конструирует Indexer (с RAG если есть эмбеддер/индекс), доказывая, что тип строится без Tauri.
    // Watcher НЕ спавним (skeleton — без fs-петли/событий UI).
    let _indexer = match (&embedder, &vectors) {
        (Some(emb), Some(vec)) => nexus_core::indexer::Indexer::with_rag(
            &db,
            root.clone(),
            emb.clone(),
            vec.clone(),
            false,
        ),
        _ => nexus_core::indexer::Indexer::new(&db, root.clone()),
    };

    // Планировщик headless: МИНИМАЛЬНЫЙ реестр. App-овский default_registry зовёт app-приватные
    // модули (contradictions/relation_reasons в GcHandler), поэтому НЕ используем его. Регистрируем
    // тривиальный health-kind (no-op) — пульс воркер-лупа — и AGENT-2 agent_run-хендлер.
    let mut registry = nexus_core::scheduler::Registry::new();
    registry.insert(
        health::KIND_HEALTH.to_string(),
        Arc::new(health::HealthHandler),
    );
    // AGENT-2: прогон цикла агента как ДОЛГОВЕЧНАЯ джоба. Хендлер держит долю db (writer/reader),
    // AIClient (agent_tools-провайдер + токенайзер/бюджет внутри).
    // defer_under_interactive()==true → уступает интерактивному LLM (S5 backpressure, глобальный гейт
    // run_due отложит agent_run, пока interactive_busy). Здесь interactive_busy всегда false (headless
    // без интерактивного чата) — гейт реально сработает в десктопе/будущей проводке.
    //
    // ✓ КОРРЕЛЯЦИЯ ЭГРЕССА (AGENT-3a, RunCtx): процесс-глобальный слот `EgressAudit.run_id` УДАЛЁН —
    // run_id ЯВНО ПРОБРАСЫВАЕТСЯ per-call как `RunCtx` (хендлер → run_agent_loop → провайдер → net).
    // Поэтому КОНКУРЕНТНЫЕ прогоны корректно атрибутируют свой эгресс независимо (у каждого свой ctx в
    // своём стеке вызова — перетереть друг друга нечем), и фоновый/параллельный egress с другим ctx не
    // путает атрибуцию. ПРЕЖНЕЕ ограничение «только строго последовательные прогоны / один egress-kind»
    // БОЛЬШЕ НЕ ДЕЙСТВУЕТ — это и был БЛОКИРУЮЩИЙ гейт, документированный AGENT-2 перед AGENT-3, теперь
    // снятый (доказано `concurrent_runs_tag_egress_independently` в agent/job.rs).
    // AGENT-MEM-1: мост к памяти. Строим всегда (degrade-safe): None-эмбеддер/индексы → recall пуст,
    // прогон стартует с голым контекстом (поведение AGENT-2). exclude_session=None — прогон не
    // привязан к chat-сессии (линковка agent_runs.session_id — поздний срез); прогон агента пишет в
    // agent_runs/memory_facts, НЕ в chat_messages/chat_episodes, так что протечь его сессии в N4b/EP
    // физически нечему. remember (Add-only) работает и без эмбеддера (пишет факт в БД).
    let agent_memory: Arc<dyn nexus_core::agent::AgentMemory> =
        Arc::new(nexus_core::agent::VaultAgentMemory::new(
            db.reader().clone(),
            db.writer().clone(),
            embedder.clone(),
            memory_vectors.clone(),
            chat_vectors.clone(),
            episode_vectors.clone(),
            None,
        ));
    registry.insert(
        nexus_core::agent::KIND_AGENT_RUN.to_string(),
        Arc::new(nexus_core::agent::AgentRunHandler::new(
            db.writer().clone(),
            db.reader().clone(),
            ai_client.clone(),
            agent_context_window,
            Some(agent_memory),
        )),
    );
    let registry = Arc::new(registry);

    // Crash-recovery НА УРОВНЕ ПРОГОНА (AGENT-2): прогоны, застрявшие в 'running' дольше TTL (приложение
    // упало во время прогона), возвращаются в 'queued' (их джобы — отдельный crash-recovery планировщика
    // requeue_running в worker_loop). Replay идемпотентен на уровне прогона (handle на терминальном — no-op)
    // и безопасен с AGENT-1 стаб-инструментами (без побочных эффектов); AGENT-3-актуатор обязан сделать
    // side-effecting инструменты идемпотентными per-op-group ДО опоры на replay (см. agent/job.rs).
    match nexus_core::agent::requeue_stale_running(
        db.writer(),
        AGENT_RUN_STALE_TTL_SECS,
        nexus_core::scheduler::now_secs(),
    )
    .await
    {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            recovered = n,
            "agent_run crash-recovery: застрявшие прогоны → queued"
        ),
        Err(e) => tracing::warn!(error = %e, "agent_run crash-recovery не удался"),
    }

    // Воркер-луп ядра с no-op-хуками: interactive_busy=false (нет интерактивного LLM в skeleton),
    // jobs_changed=() (нет UI). Shutdown-канал — наш; дроп sender'а гасит петлю (как в десктопе).
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let hooks = nexus_core::scheduler::WorkerHooks {
        interactive_busy: Box::new(|| false),
        jobs_changed: Box::new(|| {}),
    };
    let worker = tokio::spawn(nexus_core::scheduler::worker_loop(
        db.writer().clone(),
        registry.clone(),
        std::collections::HashMap::new(), // recurring: пусто (skeleton)
        db.reader().clone(),
        Vec::new(), // on_change: пусто (нет watcher-сигналов)
        hooks,
        shutdown_rx,
    ));

    let vault_name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    tracing::info!(
        vault = %root.display(),
        name = %vault_name,
        ai = ai_ready,
        embed = embed_ready,
        agent_tools = agent_ready,
        "nexus-agentd started"
    );

    let smoke = std::env::var("NEXUS_AGENTD_SMOKE").is_ok_and(|v| v == "1");
    if smoke {
        // AGENT-1 smoke: цикл агента крутится end-to-end против СТАБ-провайдера (offline, без сети) и
        // безопасного реестра (echo) — доказывает execute→feed-back→Final без живой модели/актуатора.
        agent_loop_smoke().await;
        // Smoke: ставим одну health-джобу — пульс воркера. Выход 0.
        nexus_core::scheduler::enqueue(
            db.writer(),
            health::KIND_HEALTH,
            "",
            nexus_core::scheduler::now_secs(),
            3,
        )
        .await
        .map_err(|e| format!("smoke: enqueue health: {e}"))?;

        // AGENT-2 smoke: ставим ДОЛГОВЕЧНЫЙ прогон агента через настоящий путь enqueue_agent_run
        // (строка agent_runs=queued + джоба KIND_AGENT_RUN payload=run_id). Воркер заклеймит и
        // проведёт его AgentRunHandler'ом до ТЕРМИНАЛА. Деградирует чисто: если agent_tools=None
        // (нет ai.chat в конфиге → offline-smoke), прогон финишируется 'error' ("agent tools
        // unavailable") — что всё равно ДОКАЗЫВАЕТ жизненный цикл джобы + RunCtx-проводку. Если
        // провайдер сконфигурирован и сделал эгресс — durable egress_audit-строки несут run_id.
        let run_id = nexus_core::agent::enqueue_agent_run(
            db.writer(),
            "smoke: проверь связку прогона агента",
            ai_client
                .agent_tools
                .as_ref()
                .map(|p| p.model_id())
                .or(Some("none")),
            Some("auto"),
        )
        .await
        .map_err(|e| format!("smoke: enqueue_agent_run: {e}"))?;
        tracing::info!(
            run_id,
            deadline_secs = SMOKE_TICKS_DEADLINE.as_secs(),
            "nexus-agentd: AGENT-2 smoke — прогон поставлен, крутим воркер до терминала"
        );

        // Ждём, пока воркер доведёт прогон до терминала (или дедлайн). Опрашиваем БД.
        let terminal = wait_for_terminal_run(&db, run_id, SMOKE_TICKS_DEADLINE).await;

        // Дроп sender'а гасит воркер-луп (changed()→Err→break) — graceful stop, как при закрытии vault.
        drop(shutdown_tx);
        let _ = worker.await;

        match terminal {
            Some(run) => {
                // Корреляция: сколько durable egress-строк несут этот run_id (0 в offline-smoke без модели).
                let correlated = count_egress_for_run(&db, run_id).await;
                tracing::info!(
                    run_id,
                    status = %run.status,
                    step = run.step,
                    egress_with_run_id = correlated,
                    "nexus-agentd: AGENT-2 smoke — прогон достиг терминала (lifecycle + RunCtx-корреляция проверены)"
                );
            }
            None => {
                return Err(format!(
                    "smoke: agent_run {run_id} НЕ достиг терминала за {}с (воркер не диспатчит?)",
                    SMOKE_TICKS_DEADLINE.as_secs()
                ));
            }
        }
        tracing::info!("nexus-agentd: smoke завершён (vault открыт, воркер тикал) — выход 0");
        return Ok(());
    }

    // Обычный режим: живём до Ctrl-C, затем гасим воркер и выходим.
    match tokio::signal::ctrl_c().await {
        Ok(()) => tracing::info!("nexus-agentd: получен Ctrl-C — останов"),
        Err(e) => tracing::warn!(error = %e, "nexus-agentd: ошибка ожидания Ctrl-C — останов"),
    }
    drop(shutdown_tx);
    let _ = worker.await;
    tracing::info!("nexus-agentd stopped");
    Ok(())
}

/// AGENT-2 smoke: опрашивает БД, пока прогон `run_id` не достигнет терминала ('done'/'error'/
/// 'cancelled') ИЛИ не истечёт `deadline`. Возвращает терминальный снимок или `None` (дедлайн).
/// Короткий интервал опроса (тик планировщика 5 с — прогон стаб-цикла мгновенен после клейма).
async fn wait_for_terminal_run(
    db: &Database,
    run_id: i64,
    deadline: Duration,
) -> Option<nexus_core::agent::AgentRun> {
    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(run)) = nexus_core::agent::run_store::get_run(db.reader(), run_id).await {
            if nexus_core::agent::run_store::is_terminal(&run.status) {
                return Some(run);
            }
        }
        if start.elapsed() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// AGENT-2 smoke: число durable egress_audit-строк, скоррелированных на `run_id` (доказательство
/// RunCtx-проводки, когда прогон делал эгресс; 0 в offline-smoke без сконфигурированной модели).
async fn count_egress_for_run(db: &Database, run_id: i64) -> i64 {
    db.reader()
        .query(move |c| {
            c.query_row(
                "SELECT count(*) FROM egress_audit WHERE run_id=?1",
                [run_id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(0)
}

/// Реплика `vault::load_local_config`: читает/парсит `.nexus/local.json` один раз. `None` — нет/битый.
async fn load_local_config(root: &Path) -> Option<LocalConfig> {
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .ok()?;
    LocalConfig::parse(&raw)
        .map_err(|e| tracing::warn!(error = %e, "local.json: разбор не удался — AI отключён"))
        .ok()
}

/// RAG + ПАМЯТЬ агента headless (AGENT-MEM-1): эмбеддер + note-RAG индекс + ТРИ индекса памяти
/// (переписка/факты/эпизоды). Зеркало `vault::build_rag`, но: (1) ГОНИТ `reconcile_embedding_model`
/// (CORE-2a #2) ДО открытия индексов — stale on-disk индекс под другой моделью/dim сбрасывается, иначе
/// запрос новой моделью против старого индекса → `DimMismatch`/семантический мусор; (2) открывает все
/// четыре индекса (десктоп держит их в VaultContext, agentd теперь читает память тем же эмбеддером).
struct RagBundle {
    embedder: Arc<dyn EmbeddingProvider>,
    vectors: Arc<VectorIndex>,
    chat_vectors: Arc<VectorIndex>,
    memory_vectors: Arc<VectorIndex>,
    episode_vectors: Arc<VectorIndex>,
}

async fn build_rag_min(
    db: &Database,
    root: &Path,
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<RagBundle> {
    let emb = cfg.ai.embedding.as_ref()?;
    let model = emb.model.clone().unwrap_or_else(|| "embedding".to_string());

    let dim = match emb.dim {
        Some(d) => d,
        None => {
            let probe =
                GuardedClient::for_probe(policy.clone(), audit.clone(), Duration::from_secs(30))
                    .map_err(|e| tracing::warn!(error = %e, "probe-клиент не построился — RAG off"))
                    .ok()?;
            OpenAiEmbedder::probe_dim(&probe, &emb.url, &model)
                .await
                .map_err(|e| tracing::warn!(error = %e, "проба размерности не удалась — RAG off"))
                .ok()?
        }
    };

    let guarded = GuardedClient::for_embedding(policy.clone(), audit.clone())
        .map_err(|e| tracing::warn!(error = %e, "эмбеддер не инициализирован — RAG off"))
        .ok()?;
    let embedder = OpenAiEmbedder::new(
        &guarded,
        EgressFeature::Embed,
        &emb.url,
        &model,
        dim,
        ai::default_prefixes(&model),
    );

    // CORE-2a #2: сверяем on-disk индексы с активной моделью/dim ДО открытия. Смена → сброс файлов
    // (перезаполнятся индексатором/бэкфиллом). Ошибка БД → RAG off (не открываем потенциально
    // несовместимые индексы).
    let reindex = nexus_core::vector::reconcile_embedding_model(db, root, &model, dim)
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile embedding-модели не удался — RAG off"))
        .ok()?;

    let nexus = root.join(".nexus");
    let open = |name: &str| {
        VectorIndex::open(nexus.join(name), dim)
            .map_err(
                |e| tracing::warn!(error = %e, index = name, "usearch open не удался — RAG off"),
            )
            .ok()
            .map(Arc::new)
    };
    let vectors = open("vectors.usearch")?;
    let chat_vectors = open("chat_vectors.usearch")?;
    let memory_vectors = open("memory_vectors.usearch")?;
    let episode_vectors = open("episode_vectors.usearch")?;

    tracing::info!(model = %model, dim, reindex, "RAG + память агента включены (headless)");
    Some(RagBundle {
        embedder: Arc::new(embedder),
        vectors,
        chat_vectors,
        memory_vectors,
        episode_vectors,
    })
}

/// Реплика `vault::build_chat`: пара провайдеров `(reasoning, fast)` из `ai.chat`. Доступность сервера
/// здесь не проверяем (выяснится при первом стриме) — как в app.
fn build_chat_min(
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

/// AGENT-1 (I-5): tool-capable провайдер для цикла агента. Тот же `ai.chat`-хост/модель и тот же
/// `GuardedClient::for_chat` + `EgressFeature::Chat`, что и `build_chat_min`, но ОТДЕЛЬНЫЙ тип
/// `OpenAiToolProvider` (tools НЕ протекают в chat-путь). `None` — нет `ai.chat` / клиент не построился.
fn build_agent_tools_min(
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<Arc<dyn ToolCapableProvider>> {
    let chat = cfg.ai.chat.as_ref()?;
    let model = chat.model.clone().unwrap_or_else(|| "chat".to_string());
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone())
        .map_err(|e| tracing::warn!(error = %e, "tool-провайдер агента не инициализирован"))
        .ok()?;
    let provider = OpenAiToolProvider::new(&guarded, EgressFeature::Chat, &chat.url, &model, None);
    tracing::info!(model = %model, "tool-capable провайдер агента включён (AGENT-1)");
    Some(Arc::new(provider))
}

/// AGENT-1 offline smoke: гоняет цикл агента против ФЕЙКОВОГО провайдера (без сети) и безопасного
/// реестра (echo) — ToolCalls на ходу 1, Final на ходу 2. Доказывает, что headless умеет крутить цикл
/// execute→feed-back→Final. Сети не касается (стаб-провайдер). Логирует исход; падение паникует smoke.
async fn agent_loop_smoke() {
    use nexus_core::agent::tool::{ToolCall, ToolSpec};
    use nexus_core::agent::{
        run_agent_loop, AgentEvent, EchoTool, LoopBounds, LoopOutcome, ToolRegistry,
    };
    use nexus_core::ai::tools::ToolTurn;
    use nexus_core::ai::{ChatMessage, ContextBudget};
    use nexus_core::chunker::WordTokenizer;
    use nexus_core::net::RunCtx;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    /// Стаб-провайдер: ToolCalls([echo]) → Final («ok»). Без сети.
    struct SmokeProvider {
        turns: Mutex<std::collections::VecDeque<nexus_core::ai::AiResult<ToolTurn>>>,
    }
    #[async_trait::async_trait]
    impl ToolCapableProvider for SmokeProvider {
        async fn stream_chat_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ToolSpec],
            _on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> nexus_core::ai::AiResult<ToolTurn> {
            self.turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ToolTurn::Final("ok".into())))
        }
        fn model_id(&self) -> &str {
            "smoke"
        }
    }

    let provider = SmokeProvider {
        turns: Mutex::new(
            vec![
                Ok(ToolTurn::ToolCalls(vec![ToolCall {
                    id: "s1".into(),
                    name: "debug.echo".into(),
                    arguments: r#"{"text":"agent-1 smoke"}"#.into(),
                }])),
                Ok(ToolTurn::Final("ok".into())),
            ]
            .into_iter()
            .collect(),
        ),
    };
    let mut registry = ToolRegistry::new();
    registry.insert(Arc::new(EchoTool));
    let tk = WordTokenizer;
    let cancel = Arc::new(AtomicBool::new(false));
    let mut tool_results = 0usize;
    let outcome = run_agent_loop(
        &provider,
        &registry,
        vec![ChatMessage::user("smoke: вызови echo")],
        LoopBounds::default(),
        &ContextBudget::from_context_window(Some(32768)),
        &tk,
        &cancel,
        RunCtx::NONE,
        &mut |e| {
            if matches!(e, AgentEvent::ToolResult { .. }) {
                tool_results += 1;
            }
        },
    )
    .await;
    assert!(
        matches!(outcome, LoopOutcome::Final(ref s) if s == "ok") && tool_results == 1,
        "AGENT-1 smoke: цикл должен исполнить инструмент и финализировать (получено: {outcome:?}, tool_results={tool_results})"
    );
    tracing::info!(
        "nexus-agentd: AGENT-1 smoke цикла агента пройден (execute→feed-back→Final, offline)"
    );
}

/// Реплика `vault::build_util_chat`: утилитарная мелкая модель из `ai.fast`, всегда без reasoning.
/// `None` — секции нет / клиент не построился → вызывающий делает fallback на chat_fast.
fn build_util_chat_min(
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<Arc<dyn ChatProvider>> {
    let fast = cfg.ai.fast.as_ref()?;
    let model = fast.model.clone().unwrap_or_else(|| "fast".to_string());
    let guarded = GuardedClient::for_chat(policy.clone(), audit.clone())
        .map_err(
            |e| tracing::warn!(error = %e, "ai.fast: провайдер не создан — fallback на chat_fast"),
        )
        .ok()?;
    let provider = OpenAiChatProvider::new(&guarded, EgressFeature::Chat, &fast.url, &model, None)
        .without_reasoning();
    tracing::info!(model = %model, url = %fast.url, "ai.fast (утилитарная модель) включена");
    Some(Arc::new(provider))
}
