//! nexus-core — переиспользуемое ядро Nexus (CORE-1).
//!
//! Извлечено из `nexus-desktop` (Tauri backend), чтобы будущий headless agent-service мог
//! переиспользовать те же примитивы без зависимости от Tauri/UI. Срез 1 — верифицированный
//! ЗАМКНУТЫЙ набор модулей (нет ссылок на app-only `error`/`state`/…):
//! db/parser/vector/plugin/vault/redact — листья; chunker→parser; net→plugin,redact,vault; ai→net.
//! CORE-1b — обобщённый движок планировщика (`scheduler`): tauri-free, зависит только на `db` (хуки к
//! окружению инъектируются вызывающим); app-specific spawn/handlers остаются в desktop-крейте.
//! CORE-1c-1 — кластер индекса/ретривала (`watcher`/`tags`/`tagger`/`indexer`/`graph`/`suggest`/`search`):
//! замкнутый набор (зависит только на уже-ядровые модули и друг на друга). Индексатор отвязан от Tauri —
//! watcher-петля зовёт инъектируемые [`indexer::IndexerHooks`]; desktop строит их из `AppHandle::emit`.

/// Слой актуатора (AGENT-3b): алгебра действий + PURE fail-closed classify + статус-машина + idempotency-ledger.
pub mod actuator;
/// Слой агента (AGENT-1): типизированная граница инструментов + событие-стримящий цикл + реестр инструментов.
pub mod agent;
/// AI-слой: раздельные Chat/Embedding провайдеры (ADR-005).
pub mod ai;
/// Канон сборки LLM-провайдеров из конфига (R-3a): `ProviderSet::from_config` — единый строитель
/// chat/fast/embedding/agent-tools для всех бинарей (сервер-паритет §8.3, без tauri-типов).
pub mod bootstrap;
/// Markdown-чанкер для RAG (§6.1).
pub mod chunker;
/// БД-слой: rusqlite + write-actor + read-pool (WAL) + миграции схемы (ADR-003).
pub mod db;
/// Нейтральный контракт событий хода агента (`AgentEvent` + сателлиты) — R-1: общий для agent
/// (эмиттер цикла) и actuator (Proposal/Diff), без ребра actuator→agent; `agent::event` реэкспортирует.
pub mod event;
/// Egress-граница ядра (ADR-005-ext): `GuardedClient` + политика + audit — единый chokepoint HTTP.
pub mod net;
/// Markdown-парсер (frontmatter, ссылки, теги).
pub mod parser;
/// Plugin loader (минимум): manifest + совместимость версии API (без broker — Ф2).
pub mod plugin;
/// `Redacted<T>`: безопасные Debug/Display (контент/пути не утекают в логи по неосторожности) — AC-SEC-6.
pub mod redact;
/// Транспорт-нейтральные примитивы JSON-RPC 2.0 (R-1): `RpcMessage`/`RpcError` + `Transport` +
/// line-delimited framing — общие для коннектора (`agent::connect` реэкспортирует) и песочницы.
pub mod rpc;
/// OS-песочница прогона агента (Фаза-2 каркас, `docs/specs/agent-sandbox.md`). SANDBOX-1: чистый рендер
/// хардненного `podman run` argv + конфиг (default-OFF). Рантайм/GuardedProxy/host-actuator — позже.
pub mod sandbox;
/// Планировщик фоновых задач (ADR-007) — обобщённый движок (очередь+диспатч+воркер-луп через хуки),
/// tauri-free (CORE-1b). App-specific spawn/handlers — в `crate::scheduler` desktop-крейта.
pub mod scheduler;
/// Нейтральные типы границы инструментов (`Tool`/`ToolSpec`/`ToolCall`/`ToolError`) — R-1: общие для
/// agent/actuator/ai, без рёбер actuator→agent и ai→agent; `agent::tool` реэкспортирует.
pub mod tool_types;
/// Vault: ленивый листинг + канонизация путей (анти-traversal).
pub mod vault;
/// Векторный ANN-индекс (usearch HNSW) — §6.1/§6.2.
pub mod vector;

// ── CORE-1c-1: кластер индекса/ретривала ─────────────────────────────────────────────────────────
/// Граф ссылок: беклинки из SQLite (ADR-004).
pub mod graph;
/// Инкрементальный индексатор (files/links/tags) — §4.2. Watcher-петля tauri-free (через `IndexerHooks`).
pub mod indexer;
/// Поиск по метаданным (title/path/tags) + контент-поиск (RAG) — Ф0.
pub mod search;
/// Предложения связей (режим 1 max-sim) — §6.
pub mod suggest;
/// LLM-теггер заметок (suggest_tags): словарь vault + классификация (gated `eval::classify`).
pub mod tagger;
/// Теги vault: список с количеством для панели «Теги» сайдбара (DP-2).
pub mod tags;
/// Файловый watcher (debounce + ignore + нормализация по пути).
pub mod watcher;

// ── CORE-1c-2: кластер памяти/движка ─────────────────────────────────────────────────────────────
// Замкнутый набор (зависит только на уже-ядровые модули и друг на друга, tauri-free).
/// Backup/Restore (#59): портабельный экспорт/импорт durable «второго мозга» (факты/переписка/
/// эпизоды/телеметрия скиллов) в JSON с дедупом — то, что не восстановить из vault.
pub mod backup;
/// Сессии чата в vault-БД («второй мозг» переписки, решение владельца 2026-06-12).
pub mod chat_log;
/// «Поиск противоречий» (#vision): пары-кандидаты → судья → таблица `contradictions`.
pub mod contradictions;
/// Эпизодическая память (EP): саммари завершённых чат-сессий — третий слой памяти агента.
pub mod episode;
/// Eval-харнесс качества RAG (golden + recall@k/nDCG/MRR + baseline) — §6.6. Фикстуры — `crates/nexus-core/eval/`.
pub mod eval;
/// Персистентная память агента (MEM, спека `docs/specs/agent-memory.md`): слой явных фактов + инжекция.
pub mod memory;
/// LLM-объяснения связи пары заметок (AIP-10): кэш `relation_reasons`, переиспользует примитивы `contradictions`.
pub mod relation_reasons;
/// AIP-SQ: контекстные стартовые вопросы для пустого чата (по активной заметке, best-effort).
pub mod starting_questions;

// ── SKILL-1: загрузчик SKILL.md ──────────────────────────────────────────────────────────────────
/// Загрузчик скиллов открытого стандарта SKILL.md (SKILL-1): discovery (path-scoped) + parse
/// (frontmatter БЕЗ serde_yaml, fail-closed) + каталог (single-def). Активация/инъекция/tools и
/// вендоринг/capability-гейт — SKILL-2/3 (здесь capabilities только ЗАХВАТЫВАЮТСЯ, не применяются).
pub mod skills;
