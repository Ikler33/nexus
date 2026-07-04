//! nexus-agentd — headless agent-service (CORE-2a, топология A).
//!
//! МИНИМАЛЬНЫЙ бинарь, доказывающий, что `nexus-core` переиспользуемо БЕЗ Tauri-десктопа: открывает
//! vault headless (БД + конфиг + AIClient + GuardedClient + индексатор-фундамент) и крутит воркер-луп
//! планировщика. НЕТ Tauri, НЕТ зависимостей `apps/desktop` — чистый композиционный корень над
//! публичными типами `nexus-core`. Сборка LLM-провайдеров — КАНОН
//! `nexus_core::bootstrap::ProviderSet` (R-3a; бывшие локальные реплики `build_*_min` эпохи
//! «PREFER copy over expose» удалены — политика отменена владельцем §8.8); vault-состояние RAG
//! (reconcile + открытие usearch-индексов) — модуль `rag` (`rag::build_rag_min`).
//!
//! ## Карта модулей (R-11 распил монолита `main.rs` 2120→тонкий wiring)
//! - `main.rs` — тонкий wiring: `main()` (диспетч sandbox-флагов + `run`), `run()` (композиционный
//!   корень), `build_skill_context`.
//! - `startup` — разбор env/argv, резолв app-config-dir, RESTORE owner-kill-switch'ей (egress/pause),
//!   SIGUSR1-тоггл.
//! - `rag` — vault-состояние RAG/памяти (`build_rag_min`, `RagBundle`).
//! - `connect` (unix) — AF_UNIX-хостинг коннектора (`maybe_spawn_connect_server`).
//! - `sandbox` (unix) — песочные CLI-входы (`--sandbox-child` / `--sandbox-run` / `--sandbox-undo`).
//! - `smoke` — headless smoke-харнесс (`NEXUS_AGENTD_SMOKE=1` ИЛИ cargo-feature `smoke`).
//! - `health` — тривиальный health-kind (пульс воркер-лупа).
//!
//! Запуск: `nexus-agentd <vault>` или `NEXUS_VAULT=<vault> nexus-agentd`.
//! `NEXUS_AGENTD_SMOKE=1` → один ограниченный прогон (несколько тиков) и выход 0 (headless smoke).
//!
//! ## Переменные окружения
//! - `NEXUS_VAULT` — путь к vault (если не задан `argv[1]`).
//! - `NEXUS_AGENTD_SMOKE=1` — headless smoke-прогон (несколько тиков, выход 0).
//! - `RUST_LOG` — уровень лога (`trace`/`debug`/`info`/`warn`/`error`/`off`; дефолт `info`).
//! - `NEXUS_CONFIG_DIR` — каталог app-local конфигов, где лежит egress kill-switch `egress.json`.
//!   ОБЯЗАН указывать на ТОТ ЖЕ каталог, что Tauri-десктоп использует как `app_config_dir`
//!   (`<OS config-dir>/app.nexus.desktop`) — иначе headless читает ДРУГОЙ `egress.json`, чем пишет
//!   десктоп, и kill-switch владельца молча не применяется. Если НЕ задана — берётся
//!   `<dirs::config_dir>/app.nexus.desktop` (зеркало десктопа). См. `startup::egress_config_dir`.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use nexus_core::ai::{AIClient, LocalConfig};
// Канон №2 (R-3c): бывшая локальная реплика была байт-идентична канону (включая warn-текст) — удалена.
use nexus_core::bootstrap::load_local_config;
use nexus_core::db::{Database, WriteActor};
use nexus_core::net::{EgressAudit, EgressPolicy};

#[cfg(unix)]
mod connect;
mod health;
mod rag;
#[cfg(unix)]
mod sandbox;
mod smoke;
mod startup;

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
        .with_max_level(startup::log_level_from_env())
        .init();

    // ПЕСОЧНЫЙ РЕЖИМ (SANDBOX-4b-2b): контейнер запускается как `nexus-agentd --sandbox-child …`.
    // Перехватываем ДО `run()` (он читает argv[1] как vault-путь). Тонкий in-container loop поверх 3
    // прокси на host через AF_UNIX (`run_sandbox_child_session`); host-side гейт/egress — у `SandboxRunner`.
    // Unix-only (AF_UNIX `connect_unix`); песочница — Linux-host фича (rootless-podman).
    #[cfg(unix)]
    if std::env::args().nth(1).as_deref() == Some("--sandbox-child") {
        let code = match sandbox::run_sandbox_child().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "nexus-agentd --sandbox-child: фатальная ошибка");
                1
            }
        };
        std::process::exit(code);
    }

    // HOST-РЕЖИМ ПЕСОЧНИЦЫ (SANDBOX-5): one-shot прогон ОДНОЙ задачи в хардненном контейнере. Собирает
    // `SandboxRunner` с РЕАЛЬНЫМИ backend'ами (GuardedProxy/HostActServer/event-лог) и спавнит podman.
    // `nexus-agentd --sandbox-run <vault> <task>`. Default-OFF (только по флагу) — Tier-2 live на .28.
    #[cfg(unix)]
    if std::env::args().nth(1).as_deref() == Some("--sandbox-run") {
        let code = match sandbox::run_sandbox_host().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "nexus-agentd --sandbox-run: фатальная ошибка");
                1
            }
        };
        std::process::exit(code);
    }

    // ОТКАТ exec-GitOp (SANDBOX-6c-3d-2): `nexus-agentd --sandbox-undo <vault> <run_id> [--approve]`.
    // Операторский вход: откатывает действия прогона; exec-GitOp реально reset'ит pre-op-ref в контейнере
    // (реально только при `ai.shell_enable=true` + `ai.git_worktree` + `--approve`; иначе честный Deferred). Default-safe.
    #[cfg(unix)]
    if std::env::args().nth(1).as_deref() == Some("--sandbox-undo") {
        let code = match sandbox::run_sandbox_undo().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "nexus-agentd --sandbox-undo: фатальная ошибка");
                1
            }
        };
        std::process::exit(code);
    }

    if let Err(e) = run().await {
        tracing::error!(error = %e, "nexus-agentd: фатальная ошибка");
        std::process::exit(1);
    }
}

/// Композиционный корень headless: повторяет минимум `open_vault` без Tauri/AppState.
async fn run() -> Result<(), String> {
    let raw = startup::vault_path_from_args()?;
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
    // `offline` — собственный атомик (в десктопе шарится с UI).
    //
    // CORE-2a tail (AGENT-3e §5): RESTORE персистентного `egress.json` (offline + per-feature opt-out).
    // Десктоп грузит его в lib.rs (`net::load_egress_state`→`AppState::apply_egress_state`) из Tauri
    // app_config_dir. Headless читает ТОТ ЖЕ файл из OS config-dir (зеркало десктопа), применяя offline +
    // chat/embed/probe ДО любого эгресса — так headless-agentd ЧЕСТИТ kill-switch владельца (агентский
    // Chat-эгресс ЖИВОЙ, его обязан гейтить kill-switch). Нет файла/битый → local-first-дефолты (fail-safe).
    let egress_offline = Arc::new(AtomicBool::new(false));
    let egress_policy = Arc::new(EgressPolicy::new(egress_offline.clone()));
    let egress_audit = Arc::new(EgressAudit::default());
    startup::apply_persisted_egress(&egress_offline, &egress_policy);
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

    // Сборка LLM-провайдеров — КАНОН `nexus_core::bootstrap::ProviderSet` (R-3a): chat-пара
    // (reasoning + fast) + утилитарная `ai.fast` (fallback на chat_fast) + tool-провайдер агента
    // (AGENT-1, I-5) + embedding-фундамент. Бывшие локальные реплики build_chat_min/build_util_chat_min/
    // build_agent_tools_min и embedder-часть build_rag_min удалены; байт-идентичность параметров
    // доказана характеризацией (rag::tests::boot_*, двухкоммитный приём R-2). Опции FULL: agentd строит
    // ВСЁ (агенту нечем думать без agent_tools; RAG/память — на embedding).
    let providers = match &local_cfg {
        Some(cfg) => {
            nexus_core::bootstrap::ProviderSet::from_config(
                cfg,
                &egress_policy,
                &egress_audit,
                nexus_core::bootstrap::ProviderSetOptions::FULL,
            )
            .await
        }
        None => nexus_core::bootstrap::ProviderSet::default(),
    };

    // RAG-фундамент + ПАМЯТЬ агента (AGENT-MEM-1): note-RAG индекс + ТРИ индекса памяти (переписка/
    // факты/эпизоды) поверх канонного эмбеддера. build_rag_min ГОНИТ канонный
    // reconcile_embedding_model (CORE-2a #2, R-3d «полная чистка»): stale-производные под другой
    // моделью/dim (chunks + все индексы) сбрасываются ДО открытия → нет DimMismatch на первом
    // search/upsert. Эмбеддер попадает в AIClient ТОЛЬКО из собранного бандла (reconcile/usearch
    // не удались → RAG off ЦЕЛИКОМ, эмбеддер без индексов не отдаём — как раньше).
    let rag = match providers.embedding {
        Some(eb) => rag::build_rag_min(&db, &root, eb).await,
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

    // AIClient (тот же контейнер, что десктоп кладёт в VaultContext) — собран из ядровых провайдеров.
    // AGENT-2: потребляется AgentRunHandler (нужен `agent_tools` + токенайзер/бюджет внутри хендлера),
    // поэтому в `Arc` (хендлер держит долю).
    let ai_client = Arc::new(AIClient {
        chat: providers.chat,
        chat_fast: providers.chat_fast,
        chat_util: providers.chat_util,
        embedder: embedder.clone(),
        agent_tools: providers.agent_tools,
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
    // AGENT-3e (GO-LIVE актуатора, SAFE BY DEFAULT): хендлер строит ГЕЙТНУТЫЙ реестр инструментов-
    // актуаторов ПО-ПРОГОННО — но ТОЛЬКО когда `ai.agent_actuator_enabled` ВКЛ (по умолчанию ВЫКЛ →
    // пустой реестр записи (B7), реальный vault НЕ затрагивается из коробки). Зависимости гейта:
    //  - canon_root = root (КАНОНИЗИРОВАН выше) — предусловие resolve_vault_path_for_write/apply;
    //  - overwrite_threshold / blast_cap — из конфига (дефолты ядра, если не заданы);
    //  - decision_source = PolicyDefault — HEADLESS auto-DENY: unattended agentd НИКОГДА не
    //    само-одобряет Confirm (нет UI/контрол-плейна). Даже при флаге ВКЛ headless авто-применяет
    //    лишь Auto-тир на autonomy=auto-прогоне; всякий Confirm-тир предлагается и тут же отклоняется.
    // EventSink реестра — TracingEventSink (внутри хендлера): Proposal/Diff логируются (UI-стриминг — UI-1).
    // FIXME(UI-1): связать EventSink.emit → on_event цикла / control-plane-стрим для real-time ревью
    // предложений; сегодня headless только логирует, а PolicyDefault auto-DENY-отклоняет предложения
    // (см. AgentRunHandler → actuator_registry, где TracingEventSink реально передаётся в гейт).
    let actuator_enabled = local_cfg
        .as_ref()
        .map(|c| c.ai.agent_actuator_enabled)
        .unwrap_or(false);
    let overwrite_threshold = local_cfg
        .as_ref()
        .and_then(|c| c.ai.agent_overwrite_threshold)
        .unwrap_or(nexus_core::actuator::OVERWRITE_THRESHOLD);
    let blast_cap = local_cfg
        .as_ref()
        .and_then(|c| c.ai.agent_blast_radius_cap)
        .unwrap_or(nexus_core::ai::AiConfig::DEFAULT_BLAST_RADIUS_CAP);
    // AGENT-AUTO (owner-gated 2026-06-22): автономия прогонов коннектора из `ai.agent_autonomy`.
    // ВАЛИДАЦИЯ fail-safe: только "auto" поднимает автономию, ВСЁ остальное (вкл. опечатки) → "confirm".
    let agent_autonomy_raw = local_cfg
        .as_ref()
        .and_then(|c| c.ai.agent_autonomy.as_deref());
    let agent_autonomy = if agent_autonomy_raw == Some("auto") {
        "auto"
    } else {
        "confirm"
    };
    if matches!(agent_autonomy_raw, Some(v) if v != "auto" && v != "confirm") {
        tracing::warn!(value = ?agent_autonomy_raw, "ai.agent_autonomy: неизвестное значение → fallback confirm");
    }
    if agent_autonomy == "auto" {
        tracing::warn!(
            "AGENT-AUTO: автономия коннектора = AUTO (авто-применяет Auto-тир актуатора; Confirm-тир — \
             предлагается по проводу, пишется лишь по agent/approve). Эффект при actuator_enabled."
        );
    }
    let decision_source: Arc<dyn nexus_core::actuator::DecisionSource> =
        Arc::new(nexus_core::actuator::PolicyDefault);
    if actuator_enabled {
        tracing::warn!(
            overwrite_threshold,
            blast_cap,
            "actuator GO-LIVE: файловые инструменты ВКЛ (через гейт + PolicyDefault auto-DENY) — \
             реальный vault может изменяться на autonomy=auto-прогонах (Auto-тир)"
        );
    } else {
        tracing::info!(
            "actuator GO-LIVE ВЫКЛ (safe-default): прогон агента без инструментов записи"
        );
    }
    // KILL-SWITCH (AGENT-5): process-global пауза агента. RESTORE персиста `agent.json` из app-config-dir
    // (зеркало egress kill-switch) ДО регистрации хендлера — так headless ЧЕСТИТ паузу владельца с самого
    // старта (прогоны остаются queued, цикл не идёт, актуатор не пишет). Нет файла/битый → НЕ на паузе
    // (агент работает из коробки). Arc проброшен в хендлер; рантайм-триггер — через `pause_handle()`
    // (control-plane/UI — UI-1; SIGUSR1-тоггл ниже как опциональный рантайм-вход).
    let agent_paused = Arc::new(AtomicBool::new(false));
    startup::apply_persisted_agent_pause(&agent_paused);
    // SKILL-2: контекст скиллов прогона. `ai.agent_skills_dir` задан → discovery (path-scoped) +
    // SkillContext (меню tier-1 + READ-ONLY инструменты tier-2/3). Относительный путь резолвится от
    // vault-корня (рекомендация `<vault>/.nexus/skills`). Не задан → None (агент без скиллов, без
    // регрессии). Скиллы — недоверенный внешний контент: они фенсятся в самом хендлере (I-5).
    let agent_skills = build_skill_context(local_cfg.as_ref(), &root, db.writer().clone());
    // SL-7d/SL-curator: единый owner-gated флаг `ai.skills.learning_enabled` (default false) — гейтит И
    // авторство навыков (skill.save), И фоновую курацию их жизненного цикла. Хойстим в один локал (DRY:
    // прежде вычислялся инлайн дважды в коннекторе + AgentRunHandler). `curator_skills_root` ловим ДО
    // перемещения `agent_skills` в AgentRunHandler ниже (для SkillCuratorHandler GC живого набора).
    let skills_learning_enabled = local_cfg
        .as_ref()
        .map(|c| c.ai.skills.learning_enabled)
        .unwrap_or(false);
    let curator_skills_root = agent_skills.as_ref().map(|s| s.skills_root().to_path_buf());
    // SUB-3b-2b: owner-gated делегирование (ai.delegation, default-OFF). Один конфиг → AgentRunHandler +
    // ConnectDeps (оба регистрируют delegate.run при enabled).
    let agent_delegation = local_cfg
        .as_ref()
        .map(|c| c.ai.delegation.clone())
        .unwrap_or_default();
    // RES-5: owner-gated deep-research (ai.research, default-OFF). Один конфиг → AgentRunHandler + ConnectDeps
    // (research.run регистрируется лишь при research+delegation+web+actuator+top-level).
    let agent_research = local_cfg
        .as_ref()
        .map(|c| c.ai.research.clone())
        .unwrap_or_default();

    // EGR-AGENT-2: веб-инструменты (web.search/web.fetch). Включаются ТОЛЬКО при `ai.web.enabled` —
    // `enable_web_tools` включает `EgressFeature::Web` + allowlist хоста SearXNG и строит WebToolsConfig
    // (общий клиент `for_web`). Иначе None (агент без веба). Проброс в AgentRunHandler (scheduler) И
    // коннектор (clone — Arc внутри GuardedClient дёшев). Эгресс — web-класс (SSRF-гард/allowlist/аудит).
    let agent_web = local_cfg
        .as_ref()
        .and_then(|c| c.ai.web.as_ref())
        .filter(|w| w.enabled && !w.url.trim().is_empty())
        .and_then(|w| {
            nexus_core::agent::enable_web_tools(
                &egress_policy,
                &egress_audit,
                &w.url,
                std::time::Duration::from_secs(20),
                w.allow_public_fetch, // WEB-FETCH-PUBLIC (owner-gated): web.fetch к любому публичному URL
            )
        });
    if agent_web.is_some() {
        tracing::warn!(
            "EGR-AGENT: веб-инструменты ВКЛ — web.search/web.fetch (EgressFeature::Web + allowlist SearXNG)"
        );
    }

    // AF_UNIX-хостинг коннектора (P0b-2c), default-OFF (env NEXUS_AGENTD_CONNECT_SOCKET). Делает agentd
    // ПОДКЛЮЧАЕМЫМ агент-сервисом (app↔agentd по протоколу). Тот же провайдер/память/актуатор-конфиг/веб,
    // что у AgentRunHandler — клонируем доли ДО передачи остального в хендлер ниже. AF_UNIX = локальный IPC
    // (не сетевой egress); автономия коннектора — из `ai.agent_autonomy` (default confirm; auto owner-gated).
    #[cfg(unix)]
    connect::maybe_spawn_connect_server(
        &db,
        &ai_client,
        &agent_memory,
        &root,
        actuator_enabled,
        agent_autonomy,
        overwrite_threshold,
        blast_cap,
        agent_context_window,
        &agent_skills,
        &agent_web,
        // SL-7d: owner-gated авторство навыков (ai.skills.learning_enabled, default false).
        skills_learning_enabled,
        &agent_delegation, // SUB-3b-2b: owner-gated делегирование (ai.delegation)
        &agent_research,   // RES-5: owner-gated deep-research (ai.research)
        &agent_paused,
    );

    // RES-5b: durable deep-research джоба (KIND_DEEP_RESEARCH). Те же deps, что agent_run; decision_source
    // и agent_web КЛОНИРУЕМ — AgentRunHandler::new ниже их перемещает. Default-OFF: handle финиширует error,
    // пока ai.research.enabled+web+actuator не выставлены. Сам прогон ставится enqueue_deep_research (триггер
    // — будущий UI/CLI/коннектор; сейчас kind зарегистрирован, ничего не энкьюит автоматически).
    registry.insert(
        nexus_core::agent::research::KIND_DEEP_RESEARCH.to_string(),
        Arc::new(nexus_core::agent::research::DeepResearchHandler::new(
            db.writer().clone(),
            db.reader().clone(),
            ai_client.clone(),
            root.clone(),
            actuator_enabled,
            overwrite_threshold,
            blast_cap,
            decision_source.clone(),
            agent_paused.clone(),
            agent_web.clone(),
            agent_research.clone(),
            agent_delegation.clone(),
        )),
    );

    registry.insert(
        nexus_core::agent::KIND_AGENT_RUN.to_string(),
        Arc::new(nexus_core::agent::AgentRunHandler::new(
            db.writer().clone(),
            db.reader().clone(),
            ai_client.clone(),
            agent_context_window,
            Some(agent_memory),
            root.clone(),
            actuator_enabled,
            overwrite_threshold,
            blast_cap,
            decision_source,
            agent_paused.clone(),
            agent_skills,
            agent_web,
            // SL-7d: авторство навыков (skill.save) — owner-gated ai.skills.learning_enabled (default false).
            skills_learning_enabled,
            // SUB-3b-2b: делегирование (delegate.run) — owner-gated ai.delegation (default disabled).
            agent_delegation.clone(),
            // RES-5: deep-research (research.run) — owner-gated ai.research (default disabled).
            agent_research.clone(),
        )),
    );
    // SL-curator: фоновая гигиена жизненного цикла agent-навыков (active→stale→archive, ОБРАТИМО,
    // НИКОГДА не удаляет; GC лишь орфан-телеметрии). Регистрируем ТОЛЬКО при owner-gated
    // `ai.skills.learning_enabled` И наличии skills-каталога (иначе курировать нечего → не плодим
    // no-op-джобу). Recurring/seed — ниже, тем же гейтом. `SkillCuratorHandler` ещё раз защищён внутри
    // (learning=false / root=None → sweep NOOP), defense-in-depth.
    let curator_registered = if skills_learning_enabled {
        if let Some(skills_root) = curator_skills_root.clone() {
            registry.insert(
                nexus_core::skills::curator::KIND_SKILL_CURATOR.to_string(),
                Arc::new(nexus_core::skills::curator::SkillCuratorHandler::new(
                    db.reader().clone(),
                    db.writer().clone(),
                    Some(skills_root),
                    true,
                )),
            );
            true
        } else {
            false
        }
    } else {
        false
    };
    // SL-curator (ревью #4): курация НЕ зарегистрирована (learning OFF или нет навыков) → подчистить
    // осиротевшую `skill_curator`-джобу, засиженную в ПРЕЖНЕМ ON-прогоне. Иначе при флипе learning→OFF +
    // рестарт она висела бы `pending` вечно (claim-by-kind её не заклеймит — нет хендлера; recurring/seed
    // ниже под тем же гейтом её не пересоздадут). `skill_curator` обслуживает ТОЛЬКО agentd, поэтому снос
    // безопасен (в отличие от чужих desktop-kind'ов при co-residence — их трогать нельзя).
    if !curator_registered {
        match nexus_core::scheduler::delete_jobs_of_kind(
            db.writer(),
            nexus_core::skills::curator::KIND_SKILL_CURATOR,
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!(
                    reaped = n,
                    "skill_curator: осиротевшие джобы сняты (курация выключена)"
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "skill_curator: уборка осиротевших джоб не удалась")
            }
        }
    }
    let registry = Arc::new(registry);

    // KILL-SWITCH (AGENT-5) рантайм-вход: SIGUSR1 ТОГГЛИТ паузу (опциональный сигнальный триггер — UI
    // кнопка/control-plane придут в UI-1). Чисто in-memory (персист `agent.json` пишет владелец/UI):
    // оператор headless может на лету заморозить/разморозить агента без рестарта. Только Unix.
    startup::spawn_pause_signal_toggle(agent_paused.clone());

    // Crash-recovery НА УРОВНЕ ПРОГОНА (AGENT-2): прогоны, застрявшие в 'running' дольше TTL (приложение
    // упало во время прогона), возвращаются в 'queued' (их джобы — отдельный crash-recovery планировщика
    // requeue_running в worker_loop). Replay идемпотентен на уровне прогона (handle на терминальном — no-op)
    // и безопасен при ВЫКЛ актуаторе (реестр записи пуст, B7 — без побочных эффектов); AGENT-3-актуатор обязан сделать
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

    // Crash-recovery НА УРОВНЕ ДЕЙСТВИЯ (SANDBOX-6c-3 §6): exec-строки `agent_actions`, застрявшие в
    // 'executing' (контейнер исчез ПОСЛЕ redeem APPROVED→EXECUTING, но ДО report — in-memory in_flight-карта
    // потеряна на рестарте), финализируются FAILED по TTL. НЕ requeue (exec не replay-safe: одноразовый
    // токен консьюмнут, частичный эффект мог уже случиться). Рядом с requeue_stale_running — тот же
    // канонический момент восстановления (старт, когда in_flight потеряна).
    match nexus_core::actuator::reconcile_stale_executing(
        db.writer(),
        nexus_core::actuator::EXEC_STALE_TTL_SECS,
        nexus_core::scheduler::now_secs(),
    )
    .await
    {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            reaped = n,
            "exec crash-recovery: зависшие executing-действия → failed (§6 TTL)"
        ),
        Err(e) => {
            tracing::warn!(error = %e, "exec crash-recovery (reconcile_stale_executing) не удался")
        }
    }

    // SUBAGENTS crash-recovery (SUB-3b-2b, фикс ревью #4): осиротевшие прогоны-ДЕТИ (`delegate.run`
    // fan-out, аборнутый дропом фьючи родителя НА `.await` → строка застряла `running`) финализируются
    // `error` по TTL. ЖЁСТКОЕ предусловие активации `delegate.run` — рядом с requeue_stale_running, тот же
    // канонический момент восстановления. Дети НЕ возобновляемы (не `queued`, а терминал — нет per-child replay).
    match nexus_core::agent::reconcile_orphan_child_runs(
        db.writer(),
        AGENT_RUN_STALE_TTL_SECS,
        nexus_core::scheduler::now_secs(),
    )
    .await
    {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            reaped = n,
            "subagent crash-recovery: осиротевшие прогоны-дети → error (#4 TTL)"
        ),
        Err(e) => {
            tracing::warn!(error = %e, "subagent crash-recovery (reconcile_orphan_child_runs) не удался")
        }
    }

    // Воркер-луп ядра с no-op-хуками: interactive_busy=false (нет интерактивного LLM в skeleton),
    // jobs_changed=() (нет UI). Shutdown-канал — наш; дроп sender'а гасит петлю (как в десктопе).
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let hooks = nexus_core::scheduler::WorkerHooks {
        interactive_busy: Box::new(|| false),
        jobs_changed: Box::new(|| {}),
    };
    // Recurring (slice 6): SL-curator сам переназначается раз/сутки после прогона. Только при
    // owner-gated learning + наличии skills-каталога (тем же гейтом, что регистрация хендлера выше) —
    // иначе пусто (skeleton, как прежде). Seed run-if-absent ниже даёт первый прогон до ожидания интервала.
    let mut recurring: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    if skills_learning_enabled && curator_skills_root.is_some() {
        recurring.insert(
            nexus_core::skills::curator::KIND_SKILL_CURATOR.to_string(),
            nexus_core::skills::curator::CURATOR_INTERVAL_SECS,
        );
        // Seed: ставим pending-джобу куратора ТОЛЬКО если такой ещё нет (reschedule_if_absent — не
        // стакать на каждом рестарте). run_at=now → первый прогон сразу, дальше recurring ведёт сам.
        if let Err(e) = nexus_core::scheduler::reschedule_if_absent(
            db.writer(),
            nexus_core::skills::curator::KIND_SKILL_CURATOR,
            nexus_core::scheduler::now_secs(),
            3,
        )
        .await
        {
            tracing::warn!(error = %e, "skill_curator: seed-джоба не поставлена");
        }
    }
    let worker = tokio::spawn(nexus_core::scheduler::worker_loop(
        db.writer().clone(),
        registry.clone(),
        recurring,
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

    // Smoke-режим (`NEXUS_AGENTD_SMOKE=1` ИЛИ cargo-feature `smoke`): офлайн-проверки + долговечный
    // agent_run до терминала, затем graceful-останов воркера и выход 0 (см. модуль `smoke`). Feature
    // форсит smoke на компиляции (self-testing бинарь); default-профиль — env-gated (поведение
    // сохранено). B8: smoke возвращает `Err` с диагностикой вместо `panic!` → `main` логирует и exit-1.
    let smoke = {
        #[cfg(feature = "smoke")]
        {
            true
        }
        #[cfg(not(feature = "smoke"))]
        {
            std::env::var("NEXUS_AGENTD_SMOKE").is_ok_and(|v| v == "1")
        }
    };
    if smoke {
        return smoke::run_smoke(&db, &ai_client, worker, shutdown_tx).await;
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

/// SKILL-2: строит [`nexus_core::agent::SkillContext`] прогона из `ai.agent_skills_dir`. Не задан →
/// `None` (агент без скиллов, без регрессии). Задан → канонизирует каталог (граница path-конфайна
/// tier-3) и гонит `discover_skills` (path-scoped, fail-closed; битые скиллы видимы в `errors`).
/// Относительный путь резолвится ОТ vault-корня. Каталог не существует/не каталог / нет валидных
/// скиллов → `None` (хендлер тогда работает как без скиллов). Скиллы — недоверенный контент: фенсятся
/// в хендлере (I-5).
fn build_skill_context(
    cfg: Option<&LocalConfig>,
    root: &Path,
    usage_writer: WriteActor,
) -> Option<nexus_core::agent::SkillContext> {
    let dir = cfg?.ai.agent_skills_dir.as_deref()?;
    let p = Path::new(dir);
    // Относительный путь — от vault-корня; абсолютный — как есть.
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    // Канонизируем КАТАЛОГ — это база path-конфайна ресурсов (tier 3) и корень discovery.
    let canon = match abs.canonicalize() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(skills_dir = %abs.display(), error = %e, "skills: каталог недоступен — скиллы выключены");
            return None;
        }
    };
    let catalog = nexus_core::skills::discover_skills(&canon);
    if !catalog.errors().is_empty() {
        tracing::warn!(
            count = catalog.errors().len(),
            "skills: часть скиллов битые (см. errors) — пропущены"
        );
    }
    if catalog.is_empty() {
        tracing::info!(skills_dir = %canon.display(), "skills: каталог задан, но валидных скиллов нет — скиллы выключены");
        return None;
    }
    tracing::info!(skills_dir = %canon.display(), count = catalog.len(), "skills: каталог загружен (tier-1 меню + activate_skill/read_skill_resource)");
    // SL-2: телеметрия использования скиллов ВСЕГДА-ON в проде (чистая наблюдаемость; curator/skill_save
    // будут гейтиться БУДУЩИМ флагом `ai.skills.learning_enabled` — SL-7/SL-curator, сейчас его в конфиге
    // ещё нет). Активация/чтение ресурса инкрементят `agent_skill_usage` best-effort (awaited inline,
    // дешёвый upsert, ошибка глотается).
    Some(
        nexus_core::agent::SkillContext::new(std::sync::Arc::new(catalog), canon)
            .with_usage_writer(usage_writer),
    )
}
