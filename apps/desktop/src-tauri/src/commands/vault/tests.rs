use super::*;
use crate::net::{EgressAudit, EgressFeature, EgressPolicy};
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

/// Тест-хелпер чтения `settings` (жил в проде рядом с удалённой репликой reconcile — R-3d).
async fn get_setting(db: &Database, key: &str) -> Option<String> {
    let key = key.to_string();
    db.reader()
        .query(move |c| {
            c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .optional()
        })
        .await
        .unwrap()
}

/// AC-EGR-13 (composition-root): канон `bootstrap::ProviderSet` строит провайдеров от ОДНОГО
/// policy через guarded-клиент — переключение политики мгновенно видно ВСЕМ провайдерам
/// (никаких собственных клиентов мимо chokepoint). До R-3b здесь были локальные
/// `build_chat`/`build_util_chat` — ассерты не менялись.
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
    let p = crate::bootstrap::ProviderSet::from_config(
        &cfg,
        &policy,
        &audit,
        crate::bootstrap::ProviderSetOptions {
            agent_tools: false,
            embedding: true,
        },
    )
    .await;
    let chat = p.chat.expect("chat построен");
    let chat_fast = p.chat_fast.expect("chat построен");
    let util = p.chat_util.expect("util построен");

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

// R-3d: оба reconcile-теста переключены с удалённой desktop-реплики на КАНОН
// `crate::vector::reconcile_embedding_model` — ассерты ниже НЕ менялись (канон = superset:
// first-run/no-op/чистка chunks совпадают со старой desktop-семантикой; расширение канона —
// снос chat_vectors — запинено юнит-таблицей в nexus-core `vector::tests`).

/// §6.5: первое включение RAG пишет settings и требует force; та же модель — без force.
#[tokio::test]
async fn reconcile_first_run_sets_settings_and_forces() {
    let dir = TempDir::new().unwrap();
    let db = open_db(dir.path()).await;

    let force = crate::vector::reconcile_embedding_model(&db, dir.path(), "nomic", 768)
        .await
        .unwrap();
    assert!(force, "первое включение RAG → force-переиндексация");
    assert_eq!(
        get_setting(&db, "embedding.model").await.as_deref(),
        Some("nomic")
    );
    assert_eq!(
        get_setting(&db, "embedding.dim").await.as_deref(),
        Some("768")
    );

    let again = crate::vector::reconcile_embedding_model(&db, dir.path(), "nomic", 768)
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
    crate::vector::reconcile_embedding_model(&db, root, "nomic", 768)
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

    let force = crate::vector::reconcile_embedding_model(&db, root, "bge-m3", 1024)
        .await
        .unwrap();
    assert!(force, "смена модели → force");
    assert_eq!(
        count_chunks(&db).await,
        0,
        "смена модели очистила chunks (§6.5)"
    );
    assert_eq!(
        get_setting(&db, "embedding.dim").await.as_deref(),
        Some("1024")
    );
}

// ── R-3b: ХАРАКТЕРИЗАЦИЯ сборки провайдеров open_vault (REFACTOR-PLAN §3, thermo-смелл №3) ─────
//
// Фикстура «до»: снимки ВСЕХ конфиг-наблюдаемых параметров провайдеров (`debug_params`) были
// СНЯТЫ со СТАРЫХ desktop-строителей (`build_chat`/`build_util_chat` + embedder-часть `build_rag`
// + композиция `open_vault`) в КОММИТЕ 1 этого среза (двухкоммитный приём R-2/R-3a) — и НЕ
// менялись при переключении сборки на канон `nexus_core::bootstrap::ProviderSet` (коммит 2, этот
// код): байт-идентичность канона доказана, не задекларирована.
// Строки-снимки НЕ «пере-снимать» при рефакторе канона — они и есть контракт.

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

/// Сборка ТЕКУЩИМ путём desktop — теперь это КАНОН `bootstrap::ProviderSet::from_config` с
/// опциями desktop (`agent_tools: false`), как в `open_vault` (в коммите 1 здесь была реплика
/// старой композиции `build_chat` → `build_util_chat` + fallback на chat_fast; ассерты тестов
/// НЕ менялись при переключении).
async fn build_current_way(cfg_json: &str) -> crate::bootstrap::ProviderSet {
    let cfg = boot_cfg(cfg_json);
    let (policy, audit) = boot_edges();
    crate::bootstrap::ProviderSet::from_config(
        &cfg,
        &policy,
        &audit,
        crate::bootstrap::ProviderSetOptions {
            agent_tools: false,
            embedding: true,
        },
    )
    .await
}

/// Эмбеддер ТЕКУЩИМ путём desktop: канонный `EmbeddingBootstrap` СКВОЗЬ `build_rag`
/// (reconcile+usearch на временном vault — живой RAG-путь `open_vault`; dim задан в фикстурах →
/// сетевой пробы нет). В коммите 1 тот же путь шёл через старый монолитный `build_rag`.
async fn build_embedder_current_way(cfg_json: &str) -> Option<Arc<dyn EmbeddingProvider>> {
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let db = open_db(&root).await;
    let eb = build_current_way(cfg_json).await.embedding?;
    build_rag(&root, &db, eb).await.map(|r| r.0)
}

/// Опции desktop: tool-провайдер агента НЕ строится в `open_vault` (AGENT-1, I-5) — desktop
/// строит его per-run в commands/agent.rs через канонный `ai::tools::build_agent_tool_provider`.
#[tokio::test]
async fn boot_desktop_options_hold_no_agent_tools() {
    let p = build_current_way(BOOT_CFG_FULL).await;
    assert!(p.agent_tools.is_none(), "desktop держит None (I-5)");
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

/// M2 / LLM-audit #324: recurring map строится по provider-гейтам, **без** снимка тогглов.
/// Иначе ON mid-session + успешный kick не ставит следующий суточный тик до reopen vault.
#[test]
fn recurring_map_includes_insights_and_contra_without_toggle_snapshot() {
    let (rec, on_change) = scheduler_recurring_and_on_change(
        /*chat*/ true, /*chat_util*/ true, /*vectors*/ true, /*news*/ true,
    );
    assert!(rec.contains_key(crate::digest::KIND_DIGEST));
    assert!(rec.contains_key(crate::contradictions::KIND_CONTRA));
    assert!(rec.contains_key(crate::scheduler::KIND_GC));
    assert!(rec.contains_key(&crate::home::widgets::widget_kind(
        crate::home::insights::KEY_CONTEXT_DRIFT
    )));
    assert!(rec.contains_key(&crate::home::widgets::widget_kind(
        crate::home::insights::KEY_OPEN_QUESTIONS
    )));
    assert!(rec.contains_key(crate::home::stale::KIND_STALE));
    assert!(rec.contains_key(crate::episode::KIND_EPISODE_ROLLUP));
    assert!(rec.contains_key(crate::news::KIND_NEWSFEED));
    // on_change — только digest+contra
    assert!(on_change.contains(&crate::digest::KIND_DIGEST.to_string()));
    assert!(on_change.contains(&crate::contradictions::KIND_CONTRA.to_string()));
    assert!(!on_change.iter().any(|k| k == crate::scheduler::KIND_GC));
    assert!(!on_change.iter().any(|k| k == crate::news::KIND_NEWSFEED));
    assert!(!on_change.iter().any(|k| k == crate::home::stale::KIND_STALE));
    assert_eq!(rec.get(crate::episode::KIND_EPISODE_ROLLUP), Some(&(DAY_SECS / 4)));
}

/// Без chat/util — узкий recurring (GC always; no LLM kinds).
#[test]
fn recurring_map_gc_only_without_providers() {
    let (rec, on_change) = scheduler_recurring_and_on_change(false, false, false, false);
    assert_eq!(rec.len(), 1);
    assert!(rec.contains_key(crate::scheduler::KIND_GC));
    assert!(on_change.is_empty());
}
