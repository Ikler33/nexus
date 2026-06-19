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

    // RAG-фундамент: эмбеддер + векторный индекс. Реплика build_rag МИНИМАЛЬНО — без §6.5-реконсиляции
    // модели (она трогает app-приватные settings/чистку чанков; здесь skeleton, переиндексацию не гоним).
    let rag = match &local_cfg {
        Some(cfg) => build_rag_min(&root, cfg, &egress_policy, &egress_audit).await,
        None => None,
    };
    let (vectors, embedder) = match rag {
        Some((embedder, vec_index)) => (Some(vec_index), Some(embedder)),
        None => (None, None),
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

    // AIClient (тот же контейнер, что десктоп кладёт в VaultContext) — собран из ядровых провайдеров.
    // Здесь не потребляется логикой (skeleton), но доказывает, что headless собирает полный AI-слой.
    let ai_client = AIClient {
        chat,
        chat_fast,
        chat_util,
        embedder: embedder.clone(),
        policy: egress_policy.clone(),
    };
    let ai_ready = ai_client.chat.is_some();
    let embed_ready = ai_client.embedder.is_some();

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
    // тривиальный health-kind (no-op) — доказательство, что воркер-луп тикает и диспатчит.
    let mut registry = nexus_core::scheduler::Registry::new();
    registry.insert(
        health::KIND_HEALTH.to_string(),
        Arc::new(health::HealthHandler),
    );
    let registry = Arc::new(registry);

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
        "nexus-agentd started"
    );

    let smoke = std::env::var("NEXUS_AGENTD_SMOKE").is_ok_and(|v| v == "1");
    if smoke {
        // Smoke: ставим одну health-джобу, ждём ограниченное время (даём воркеру тикнуть), стоп. Выход 0.
        nexus_core::scheduler::enqueue(
            db.writer(),
            health::KIND_HEALTH,
            "",
            nexus_core::scheduler::now_secs(),
            3,
        )
        .await
        .map_err(|e| format!("smoke: enqueue health: {e}"))?;
        tracing::info!(
            deadline_secs = SMOKE_TICKS_DEADLINE.as_secs(),
            "nexus-agentd: smoke-режим — крутим воркер до дедлайна, затем выход 0"
        );
        tokio::time::sleep(SMOKE_TICKS_DEADLINE).await;
        // Дроп sender'а гасит воркер-луп (changed()→Err→break) — graceful stop, как при закрытии vault.
        drop(shutdown_tx);
        let _ = worker.await;
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

/// Реплика `vault::load_local_config`: читает/парсит `.nexus/local.json` один раз. `None` — нет/битый.
async fn load_local_config(root: &Path) -> Option<LocalConfig> {
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .ok()?;
    LocalConfig::parse(&raw)
        .map_err(|e| tracing::warn!(error = %e, "local.json: разбор не удался — AI отключён"))
        .ok()
}

/// МИНИМАЛЬНАЯ реплика `vault::build_rag`: эмбеддер + основной векторный индекс. Опущено относительно
/// app: §6.5-реконсиляция модели и параллельные индексы памяти (chat/memory/episode — не нужны skeleton'у).
/// Размерность: из конфига или пробой у сервера. ВАЖНО: без реконсиляции существующий
/// `.nexus/vectors.usearch`, записанный ПОД ДРУГОЙ моделью/dim (прошлый прогон десктопа), открывается
/// как есть → рассинхрон всплывёт как `DimMismatch` на ПЕРВОМ search/upsert. Безопасно ТОЛЬКО потому, что
/// skeleton индекс не читает/не пишет; RAG-запрашивающий agentd ОБЯЗАН сперва прогнать
/// `reconcile_embedding_model` (или эквивалентный dim/model-гард).
async fn build_rag_min(
    root: &Path,
    cfg: &LocalConfig,
    policy: &Arc<EgressPolicy>,
    audit: &Arc<EgressAudit>,
) -> Option<(Arc<dyn EmbeddingProvider>, Arc<VectorIndex>)> {
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

    let vectors = VectorIndex::open(root.join(".nexus").join("vectors.usearch"), dim)
        .map_err(|e| tracing::warn!(error = %e, "usearch open не удался — RAG off"))
        .ok()?;

    tracing::info!(model = %model, dim, "RAG включён (headless)");
    Some((Arc::new(embedder), Arc::new(vectors)))
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
