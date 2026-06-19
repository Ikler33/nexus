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
//!
//! ## Переменные окружения
//! - `NEXUS_VAULT` — путь к vault (если не задан `argv[1]`).
//! - `NEXUS_AGENTD_SMOKE=1` — headless smoke-прогон (несколько тиков, выход 0).
//! - `RUST_LOG` — уровень лога (`trace`/`debug`/`info`/`warn`/`error`/`off`; дефолт `info`).
//! - `NEXUS_CONFIG_DIR` — каталог app-local конфигов, где лежит egress kill-switch `egress.json`.
//!   ОБЯЗАН указывать на ТОТ ЖЕ каталог, что Tauri-десктоп использует как `app_config_dir`
//!   (`<OS config-dir>/app.nexus.desktop`) — иначе headless читает ДРУГОЙ `egress.json`, чем пишет
//!   десктоп, и kill-switch владельца молча не применяется. Если НЕ задана — берётся
//!   `<dirs::config_dir>/app.nexus.desktop` (зеркало десктопа). См. [`egress_config_dir`].

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

/// Сегмент каталога bundle-id, под которым ОБА kill-switch'а (egress.json И agent.json) живут в OS
/// config-dir. ЕДИНЫЙ источник истины (AGENT-5): дублировался бы в каждом резолве config-dir, и при
/// ребрендинге легко рассинхронизировать headless-чтение с десктоп-записью. Держим в ОДНОЙ константе —
/// ребрендинг меняет одно место.
///
/// ⚠️ ОБЯЗАН СОВПАДАТЬ с Tauri `identifier` в `tauri.conf.json` (`app.nexus.desktop`): десктоп пишет
/// `egress.json`/`agent.json` в `<OS config-dir>/<identifier>`, а headless читает их из
/// `<OS config-dir>/NEXUS_BUNDLE_DIR`. Если identifier поменяется, а эта строка — нет, headless будет
/// читать ДРУГОЙ файл, чем пишет десктоп → kill-switch'и владельца (offline / пауза агента) молча
/// неэффективны. См. [`egress_config_dir`].
const NEXUS_BUNDLE_DIR: &str = "app.nexus.desktop";

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

/// Каталог app-local конфигов (где живёт `egress.json`) — зеркало того, что десктоп получает из Tauri
/// `app_config_dir` (`<OS config-dir>/<identifier>`). Порядок: env `NEXUS_CONFIG_DIR` (явное
/// переопределение / тесты) → `<dirs::config_dir>/app.nexus.desktop` (тот же файл, что пишет десктоп) →
/// `None`, если OS config-dir не определён (тогда kill-switch грузить неоткуда — local-first-дефолты).
///
/// ## КОНТРАКТ (AGENT-3e Fix-4) — kill-switch должен читать ТОТ ЖЕ файл, что пишет десктоп
/// Разрешённый каталог ОБЯЗАН совпадать с Tauri-десктопным `app_config_dir`. Десктоп пишет `egress.json`
/// (и `agent.json`) в `<OS config-dir>/<bundle identifier>` — а identifier берётся из `tauri.conf.json`
/// (`app.nexus.desktop`). Здесь сегмент берётся из ЕДИНОЙ константы [`NEXUS_BUNDLE_DIR`] (де-дуп —
/// AGENT-5), которая ОБЯЗАНА совпадать с тем identifier. ЕСЛИ identifier в конфиге десктопа изменится
/// (ребрендинг/смена bundle id), а [`NEXUS_BUNDLE_DIR`] — нет, headless будет читать ДРУГОЙ файл, чем
/// пишет десктоп: владелец жмёт «offline»/ставит агента на паузу в UI, десктоп пишет в свой каталог, а
/// agentd грузит local-first-дефолты из несуществующего/другого файла → **kill-switch молча неэффективен**
/// (headless продолжит эгресс/работу). Поэтому при смене bundle identifier ОБЯЗАТЕЛЬНО менять и
/// [`NEXUS_BUNDLE_DIR`] (либо задавать `NEXUS_CONFIG_DIR` явно на тот же каталог). `NEXUS_CONFIG_DIR` —
/// штатный способ переопределить локацию (нестандартный config-dir / контейнер / тест), указывая на
/// каталог десктопа.
fn egress_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NEXUS_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    dirs::config_dir().map(|d| d.join(NEXUS_BUNDLE_DIR))
}

/// CORE-2a tail (AGENT-3e §5): RESTORE персистентного egress kill-switch. Грузит `egress.json` из
/// app-config-dir (зеркало `AppState::apply_egress_state` десктопа) и применяет: `offline` → общий
/// атомик политики; chat/embed/probe → per-feature opt-out. Нет файла/нет config-dir → local-first-
/// дефолты (политика уже построена с offline=false + фичи ON). Логирует применённое (наблюдаемость).
fn apply_persisted_egress(egress_offline: &Arc<AtomicBool>, policy: &Arc<EgressPolicy>) {
    let Some(dir) = egress_config_dir() else {
        tracing::info!(
            "egress.json: OS config-dir не определён — kill-switch local-first (дефолты)"
        );
        return;
    };
    apply_egress_from_dir(&dir, egress_offline, policy);
}

/// Применить `egress.json` из КОНКРЕТНОГО каталога (разделено из [`apply_persisted_egress`] для тестов
/// без зависимости от env/OS config-dir). offline → общий с политикой атомик; chat/embed/probe →
/// per-feature opt-out. Нет файла/битый → local-first-дефолты (`net::persist::load`).
fn apply_egress_from_dir(dir: &Path, egress_offline: &Arc<AtomicBool>, policy: &Arc<EgressPolicy>) {
    let path = dir.join("egress.json");
    let existed = path.exists();
    let st = nexus_core::net::load_egress_state(&path);
    // offline — общий с политикой атомик (политика читает его в check()).
    egress_offline.store(st.offline, std::sync::atomic::Ordering::Relaxed);
    policy.set_feature_enabled(EgressFeature::Chat, st.chat);
    policy.set_feature_enabled(EgressFeature::Embed, st.embed);
    policy.set_feature_enabled(EgressFeature::Probe, st.probe);
    if existed {
        tracing::info!(
            path = %path.display(),
            offline = st.offline,
            chat = st.chat,
            embed = st.embed,
            probe = st.probe,
            "egress.json восстановлен — kill-switch владельца применён (headless)"
        );
    } else {
        tracing::info!(
            path = %path.display(),
            "egress.json отсутствует — kill-switch local-first (дефолты: online, фичи ON)"
        );
    }
}

/// KILL-SWITCH (AGENT-5): RESTORE персистентной паузы агента. Грузит `agent.json` из app-config-dir
/// (ТОТ ЖЕ каталог, что egress.json — зеркало десктопа/[`egress_config_dir`]) и взводит общий атомик,
/// если `paused=true`. Нет файла/нет config-dir → НЕ на паузе (агент работает из коробки). Логирует.
fn apply_persisted_agent_pause(agent_paused: &Arc<AtomicBool>) {
    let Some(dir) = egress_config_dir() else {
        tracing::info!(
            "agent.json: OS config-dir не определён — kill-switch агента local-first (не на паузе)"
        );
        return;
    };
    apply_agent_pause_from_dir(&dir, agent_paused);
}

/// Применить `agent.json` из КОНКРЕТНОГО каталога (разделено для тестов без env/OS config-dir). Нет
/// файла/битый → дефолт (не на паузе). Зеркало [`apply_egress_from_dir`].
fn apply_agent_pause_from_dir(dir: &Path, agent_paused: &Arc<AtomicBool>) {
    let path = dir.join("agent.json");
    let existed = path.exists();
    let st = nexus_core::agent::load_control_state(&path);
    agent_paused.store(st.paused, std::sync::atomic::Ordering::Relaxed);
    if existed {
        tracing::info!(
            path = %path.display(),
            paused = st.paused,
            "agent.json восстановлен — kill-switch агента применён (headless)"
        );
    } else {
        tracing::info!(
            path = %path.display(),
            "agent.json отсутствует — kill-switch агента local-first (не на паузе)"
        );
    }
}

/// KILL-SWITCH (AGENT-5) рантайм-вход (Unix): SIGUSR1 ТОГГЛИТ `agent_paused` (in-memory). Опциональный
/// сигнальный триггер для headless-оператора (UI-кнопка/control-plane — UI-1). На не-Unix — no-op.
fn spawn_pause_signal_toggle(agent_paused: Arc<AtomicBool>) {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
            Ok(mut sig) => {
                tokio::spawn(async move {
                    while sig.recv().await.is_some() {
                        let was =
                            agent_paused.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                        tracing::warn!(
                            paused = !was,
                            "kill-switch агента ТОГГЛНУТ по SIGUSR1 (рантайм)"
                        );
                    }
                });
            }
            Err(e) => tracing::warn!(error = %e, "SIGUSR1-тоггл паузы не подключён"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = agent_paused; // не-Unix: рантайм-сигнала нет (UI-1 даст кросс-платформенный вход)
    }
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
    apply_persisted_egress(&egress_offline, &egress_policy);
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
    // AGENT-3e (GO-LIVE актуатора, SAFE BY DEFAULT): хендлер строит ГЕЙТНУТЫЙ реестр инструментов-
    // актуаторов ПО-ПРОГОННО — но ТОЛЬКО когда `ai.agent_actuator_enabled` ВКЛ (по умолчанию ВЫКЛ →
    // стабы, реальный vault НЕ затрагивается из коробки). Зависимости гейта:
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
        tracing::info!("actuator GO-LIVE ВЫКЛ (safe-default): прогон агента — только стабы");
    }
    // KILL-SWITCH (AGENT-5): process-global пауза агента. RESTORE персиста `agent.json` из app-config-dir
    // (зеркало egress kill-switch) ДО регистрации хендлера — так headless ЧЕСТИТ паузу владельца с самого
    // старта (прогоны остаются queued, цикл не идёт, актуатор не пишет). Нет файла/битый → НЕ на паузе
    // (агент работает из коробки). Arc проброшен в хендлер; рантайм-триггер — через `pause_handle()`
    // (control-plane/UI — UI-1; SIGUSR1-тоггл ниже как опциональный рантайм-вход).
    let agent_paused = Arc::new(AtomicBool::new(false));
    apply_persisted_agent_pause(&agent_paused);
    // SKILL-2: контекст скиллов прогона. `ai.agent_skills_dir` задан → discovery (path-scoped) +
    // SkillContext (меню tier-1 + READ-ONLY инструменты tier-2/3). Относительный путь резолвится от
    // vault-корня (рекомендация `<vault>/.nexus/skills`). Не задан → None (агент без скиллов, без
    // регрессии). Скиллы — недоверенный внешний контент: они фенсятся в самом хендлере (I-5).
    let agent_skills = build_skill_context(local_cfg.as_ref(), &root);
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
        )),
    );
    let registry = Arc::new(registry);

    // KILL-SWITCH (AGENT-5) рантайм-вход: SIGUSR1 ТОГГЛИТ паузу (опциональный сигнальный триггер — UI
    // кнопка/control-plane придут в UI-1). Чисто in-memory (персист `agent.json` пишет владелец/UI):
    // оператор headless может на лету заморозить/разморозить агента без рестарта. Только Unix.
    spawn_pause_signal_toggle(agent_paused.clone());

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
        // AGENT-3e smoke: actuator GO-LIVE ЧЕРЕЗ ГЕЙТ (offline, без сети) — доказывает, что включённый
        // флаг + autonomy=auto + Auto-тир note.create реально пишет в vault ИМЕННО через
        // dispatch_action (ledger Executed), а PolicyDefault не препятствует Auto-тиру. Использует
        // СВОЙ временный vault (не трогает целевой root) и фейк-провайдер (без модели/сети).
        actuator_gate_smoke().await;
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

/// SKILL-2: строит [`SkillContext`] прогона из `ai.agent_skills_dir`. Не задан → `None` (агент без
/// скиллов, без регрессии). Задан → канонизирует каталог (граница path-конфайна tier-3) и гонит
/// `discover_skills` (path-scoped, fail-closed; битые скиллы видимы в `errors`). Относительный путь
/// резолвится ОТ vault-корня. Каталог не существует/не каталог / нет валидных скиллов → `None`
/// (хендлер тогда работает как без скиллов). Скиллы — недоверенный контент: фенсятся в хендлере (I-5).
fn build_skill_context(
    cfg: Option<&LocalConfig>,
    root: &Path,
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
    Some(nexus_core::agent::SkillContext::new(
        std::sync::Arc::new(catalog),
        canon,
    ))
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
    let agent_paused = Arc::new(AtomicBool::new(false));
    let mut tool_results = 0usize;
    let outcome = run_agent_loop(
        &provider,
        &registry,
        vec![ChatMessage::user("smoke: вызови echo")],
        LoopBounds::default(),
        &ContextBudget::from_context_window(Some(32768)),
        &tk,
        &cancel,
        &agent_paused,
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

/// AGENT-3e ФЕЙК-провайдер: ход 1 — ToolCalls([note.create]); ход 2 — Final. Без сети/модели —
/// детерминированно скриптует один `note.create` (Auto-тир), доказывая живой путь tool→dispatch_action
/// →apply offline. Совместно используется headless-smoke ([`actuator_gate_smoke`]) и CI-тестом
/// ([`tests::live_actuator_gate_applies_via_gate`]) — единый источник проводки, без дублирования.
struct CreateThenFinalProvider {
    turns:
        std::sync::Mutex<std::collections::VecDeque<nexus_core::ai::AiResult<ai::tools::ToolTurn>>>,
}

impl CreateThenFinalProvider {
    /// Скрипт «создать `rel` с телом `content`, затем Final». Эмитит ОДИН note.create-tool_call.
    /// `rel`/`content` — простые тестовые значения без JSON-спецсимволов (кавычек/бэкслэшей), поэтому
    /// собираем args прямым `format!` — nexus-agentd намеренно БЕЗ `serde_json` (минимум зависимостей).
    fn note_create(rel: &str, content: &str) -> Self {
        use ai::tools::ToolTurn;
        use nexus_core::agent::tool::ToolCall;
        let args = format!(r#"{{"path":"{rel}","content":"{content}"}}"#);
        Self {
            turns: std::sync::Mutex::new(
                vec![
                    Ok(ToolTurn::ToolCalls(vec![ToolCall {
                        id: "n1".into(),
                        name: "note.create".into(),
                        arguments: args,
                    }])),
                    Ok(ToolTurn::Final("готово".into())),
                ]
                .into_iter()
                .collect(),
            ),
        }
    }
}

#[async_trait::async_trait]
impl ToolCapableProvider for CreateThenFinalProvider {
    async fn stream_chat_tools(
        &self,
        _m: &[ai::ChatMessage],
        _t: &[nexus_core::agent::tool::ToolSpec],
        _o: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
        _ctx: nexus_core::net::RunCtx,
    ) -> nexus_core::ai::AiResult<ai::tools::ToolTurn> {
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(ai::tools::ToolTurn::Final("ok".into())))
    }
    fn model_id(&self) -> &str {
        "actuator-gate-fake"
    }
}

/// Результат прогона actuator-гейта в temp-vault: что записано на диск + сколько executed-строк ledger
/// + терминальный статус прогона. Используется и smoke-, и тест-вызывателем для ассертов.
struct ActuatorGateResult {
    written: Option<String>,
    executed: i64,
    status: Option<String>,
}

/// AGENT-3e ЖИВОЙ путь актуатора ЧЕРЕЗ ГЕЙТ (offline, без сети/модели) — ЕДИНЫЙ движок для headless-
/// smoke и CI-теста. Строит ВКЛЮЧЁННЫЙ [`AgentRunHandler`] (`actuator_enabled=true`,
/// `decision_source=PolicyDefault`) над уже-открытым `db` с КАНОНИЗИРОВАННЫМ `canon_root` и фейк-
/// провайдером, скриптующим `note.create` (Auto-тир). autonomy=auto → гейт авто-применяет Auto-тир
/// напрямую: файл записан, ledger executed, classify_hash протянут. Возвращает [`ActuatorGateResult`]
/// (вызыватель ассертит). Реальный vault пользователя НЕ трогаем — caller даёт временный root.
async fn drive_actuator_gate_run(
    canon_root: &Path,
    db: &Database,
    rel: &str,
    content: &str,
) -> ActuatorGateResult {
    use nexus_core::agent::{enqueue_agent_run, run_store, AgentRunHandler, KIND_AGENT_RUN};
    use nexus_core::net::EgressPolicy;
    use nexus_core::scheduler::{Job, JobHandler};

    let provider = Arc::new(CreateThenFinalProvider::note_create(rel, content));
    let ai = Arc::new(AIClient {
        chat: None,
        chat_fast: None,
        chat_util: None,
        embedder: None,
        agent_tools: Some(provider),
        policy: Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false)))),
    });
    let handler = AgentRunHandler::new(
        db.writer().clone(),
        db.reader().clone(),
        ai,
        Some(32768),
        None,
        canon_root.to_path_buf(),
        true, // actuator ВКЛ (go-live флаг)
        nexus_core::actuator::OVERWRITE_THRESHOLD,
        nexus_core::ai::AiConfig::DEFAULT_BLAST_RADIUS_CAP,
        // PolicyDefault: НЕ спрашивается для Auto-тира в auto-прогоне (применяется напрямую под кэпом);
        // подтверждает, что go-live-проводка применяет Auto-тир, а не блокирует его auto-DENY.
        Arc::new(nexus_core::actuator::PolicyDefault),
        // KILL-SWITCH (AGENT-5): smoke/CI-путь — kill-switch НЕ взведён (проверяем go-live apply).
        Arc::new(AtomicBool::new(false)),
        // SKILL-2: actuator-gate smoke не про скиллы → без skills.
        None,
    );

    let run_id = enqueue_agent_run(
        db.writer(),
        "создай заметку",
        Some("actuator-gate-fake"),
        Some("auto"),
    )
    .await
    .expect("enqueue_agent_run");
    let job = Job {
        id: 1,
        kind: KIND_AGENT_RUN.into(),
        payload: run_id.to_string(),
        state: "running".into(),
        run_at: 0,
        attempts: 0,
        max_attempts: 3,
        last_error: None,
    };
    handler.handle(&job).await.expect("actuator run");

    let written = std::fs::read_to_string(canon_root.join(rel)).ok();
    let executed: i64 = db
        .reader()
        .query(move |c| {
            c.query_row(
                "SELECT count(*) FROM agent_actions WHERE run_id=?1 AND state='executed'",
                [run_id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(-1);
    let status = run_store::get_run(db.reader(), run_id)
        .await
        .ok()
        .flatten()
        .map(|r| r.status);
    ActuatorGateResult {
        written,
        executed,
        status,
    }
}

/// AGENT-3e offline smoke: actuator GO-LIVE ЧЕРЕЗ ГЕЙТ. Открывает СВОЙ временный vault и гоняет
/// [`drive_actuator_gate_run`] (флаг ВКЛ + autonomy=auto + `note.create`). Доказывает живую проводку
/// tool→dispatch_action→apply БЕЗ модели/сети. Падение — паника (валит smoke): это акцептанс go-live.
/// Целевой root НЕ трогаем (свой temp vault). CI-эквивалент — [`tests::live_actuator_gate_applies_via_gate`].
async fn actuator_gate_smoke() {
    let dir = std::env::temp_dir().join(format!("nexus-actuator-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("smoke: temp vault");
    let canon_root = dir.canonicalize().expect("smoke: canonicalize vault");
    let db = Database::open(canon_root.join(".nexus").join("nexus.db"))
        .await
        .expect("smoke: open db");

    let res = drive_actuator_gate_run(&canon_root, &db, "Notes/Smoke.md", "создано гейтом").await;
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        res.written.as_deref(),
        Some("создано гейтом"),
        "AGENT-3e smoke: note.create ДОЛЖНА быть записана ЧЕРЕЗ ГЕЙТ (флаг ВКЛ, autonomy=auto)"
    );
    assert_eq!(
        res.executed, 1,
        "AGENT-3e smoke: ровно одна executed apply-строка ledger (apply через dispatch_action)"
    );
    tracing::info!(
        status = res.status.as_deref().unwrap_or("?"),
        "nexus-agentd: AGENT-3e actuator smoke пройден (tool→dispatch_action→apply, ledger executed, offline)"
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

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::net::{EgressDenied, EgressState};
    use tempfile::TempDir;

    /// Свежая политика + общий offline-атомик (как в `run()`).
    fn fresh_policy() -> (Arc<AtomicBool>, Arc<EgressPolicy>) {
        let offline = Arc::new(AtomicBool::new(false));
        let policy = Arc::new(EgressPolicy::new(offline.clone()));
        (offline, policy)
    }

    /// **AGENT-3e Fix-1 (HIGH — CI покрывает ЖИВОЙ write-путь актуатора).** Гоняет тот же движок, что
    /// headless-smoke ([`drive_actuator_gate_run`]), но как `#[tokio::test]` — поэтому
    /// `cargo test -p nexus-agentd` (CI через `--workspace`) теперь упражняет полную проводку
    /// `tool → dispatch_action → apply` НА УРОВНЕ agentd, а не только за рантайм-флагом
    /// `NEXUS_AGENTD_SMOKE=1`. Доказывает: ВКЛЮЧЁННЫЙ флаг актуатора + `autonomy=auto` + Auto-тир
    /// `note.create` → файл реально записан в vault + ровно одна `executed` apply-строка ledger (apply
    /// прошёл ЧЕРЕЗ ГЕЙТ — `dispatch_action`, не в обход) + classify_hash протянут (иначе drift-рубеж
    /// отменил бы запись). Полностью ОФЛАЙН: фейк-провайдер ([`CreateThenFinalProvider`]) скриптует ходы
    /// без модели/сети; vault — `TempDir` (целевой root пользователя не трогаем). Регрессия, ломающая
    /// живой apply-путь, теперь ВАЛИТ CI (раньше прошла бы все гейты — это и был пробел go-live-ревью).
    #[tokio::test]
    async fn live_actuator_gate_applies_via_gate() {
        let dir = TempDir::new().unwrap();
        // canon_root КАНОНИЗИРОВАН — предусловие гейта/apply (на macOS /tmp → /private/tmp).
        let canon_root = dir.path().canonicalize().unwrap();
        let db = Database::open(canon_root.join(".nexus").join("nexus.db"))
            .await
            .unwrap();

        let res =
            drive_actuator_gate_run(&canon_root, &db, "Notes/Gate.md", "создано гейтом (CI)").await;

        assert_eq!(
            res.written.as_deref(),
            Some("создано гейтом (CI)"),
            "флаг ВКЛ + autonomy=auto + Auto-тир: note.create записана ЧЕРЕЗ ГЕЙТ (dispatch_action→apply)"
        );
        assert_eq!(
            res.executed, 1,
            "ровно одна executed apply-строка ledger (apply прошёл через гейт, classify_hash протянут)"
        );
        assert_eq!(
            res.status.as_deref(),
            Some("done"),
            "прогон дошёл до терминала done после применённого действия"
        );
        // Vault внутри TempDir — дроп `dir` чистит за собой; никакого egress (фейк-провайдер).
    }

    /// **CORE-2a tail (AGENT-3e §5): persisted offline=ON ЧЕСТИТСЯ agentd.** Сохраняем egress.json с
    /// offline=true, применяем — политика ДЕНАИТ публичный хост (Offline), но LAN/loopback живут.
    #[test]
    fn persisted_offline_is_honored() {
        let dir = TempDir::new().unwrap();
        nexus_core::net::save_egress_state(
            &dir.path().join("egress.json"),
            &EgressState {
                offline: true,
                chat: true,
                embed: true,
                probe: true,
            },
        )
        .unwrap();

        let (offline, policy) = fresh_policy();
        apply_egress_from_dir(dir.path(), &offline, &policy);

        assert!(
            offline.load(std::sync::atomic::Ordering::Relaxed),
            "offline применён"
        );
        // Публичный хост отрезан kill-switch'ем.
        assert_eq!(
            policy.check("api.example.com", EgressFeature::Chat),
            Err(EgressDenied::Offline),
            "offline=ON: публичный Chat-хост денайнут (kill-switch уважён)"
        );
        // LAN/loopback живут даже в офлайне (local-first).
        assert!(
            policy.check("127.0.0.1", EgressFeature::Chat).is_ok(),
            "loopback живёт в офлайне"
        );
    }

    /// Per-feature opt-out из egress.json ЧЕСТИТСЯ: chat=false → Chat-фича выключена даже к loopback.
    #[test]
    fn persisted_feature_optout_is_honored() {
        let dir = TempDir::new().unwrap();
        nexus_core::net::save_egress_state(
            &dir.path().join("egress.json"),
            &EgressState {
                offline: false,
                chat: false,
                embed: true,
                probe: true,
            },
        )
        .unwrap();

        let (offline, policy) = fresh_policy();
        apply_egress_from_dir(dir.path(), &offline, &policy);

        assert!(
            !policy.is_feature_enabled(EgressFeature::Chat),
            "chat opt-out применён"
        );
        assert_eq!(
            policy.check("127.0.0.1", EgressFeature::Chat),
            Err(EgressDenied::FeatureNotEnabled(EgressFeature::Chat)),
            "chat=false: даже loopback Chat выключен"
        );
        assert!(
            policy.check("127.0.0.1", EgressFeature::Embed).is_ok(),
            "embed остался ON"
        );
    }

    /// Нет файла → local-first-дефолты (online, фичи ON) — fail-safe, не валит старт.
    #[test]
    fn missing_egress_json_is_local_first_defaults() {
        let dir = TempDir::new().unwrap(); // пуст, файла нет
        let (offline, policy) = fresh_policy();
        apply_egress_from_dir(dir.path(), &offline, &policy);

        assert!(
            !offline.load(std::sync::atomic::Ordering::Relaxed),
            "online по умолчанию"
        );
        assert!(policy.is_feature_enabled(EgressFeature::Chat));
        assert!(policy.is_feature_enabled(EgressFeature::Embed));
        assert!(policy.is_feature_enabled(EgressFeature::Probe));
    }

    /// `NEXUS_CONFIG_DIR` переопределяет локацию (явный путь приоритетнее OS config-dir).
    /// Env-тест изолирован (один тест трогает env; остальные не зависят от него).
    #[test]
    fn config_dir_env_override() {
        std::env::set_var("NEXUS_CONFIG_DIR", "/tmp/nexus-test-cfg-xyz");
        assert_eq!(
            egress_config_dir(),
            Some(PathBuf::from("/tmp/nexus-test-cfg-xyz"))
        );
        std::env::remove_var("NEXUS_CONFIG_DIR");
    }

    // ── AGENT-5: KILL-SWITCH персист (agent.json restore) ─────────────────────────────────────────

    /// **persisted paused=ON ЧЕСТИТСЯ agentd.** Сохраняем agent.json с paused=true, применяем —
    /// общий атомик kill-switch взведён (хендлер увидит паузу с самого старта).
    #[test]
    fn persisted_agent_pause_is_honored() {
        let dir = TempDir::new().unwrap();
        nexus_core::agent::save_control_state(
            &dir.path().join("agent.json"),
            &nexus_core::agent::AgentControlState { paused: true },
        )
        .unwrap();

        let agent_paused = Arc::new(AtomicBool::new(false));
        apply_agent_pause_from_dir(dir.path(), &agent_paused);
        assert!(
            agent_paused.load(std::sync::atomic::Ordering::Relaxed),
            "persisted paused=true применён (kill-switch агента взведён)"
        );
    }

    /// Нет agent.json (первый запуск) → НЕ на паузе (агент работает из коробки) — fail-safe старта.
    #[test]
    fn missing_agent_json_is_not_paused() {
        let dir = TempDir::new().unwrap(); // пуст
        let agent_paused = Arc::new(AtomicBool::new(false));
        apply_agent_pause_from_dir(dir.path(), &agent_paused);
        assert!(
            !agent_paused.load(std::sync::atomic::Ordering::Relaxed),
            "нет файла → агент НЕ на паузе (работает из коробки)"
        );
    }
}
