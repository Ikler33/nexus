//! nexus-core — переиспользуемое ядро Nexus (CORE-1).
//!
//! Извлечено из `nexus-desktop` (Tauri backend), чтобы будущий headless agent-service мог
//! переиспользовать те же примитивы без зависимости от Tauri/UI. Срез 1 — верифицированный
//! ЗАМКНУТЫЙ набор модулей (нет ссылок на app-only `error`/`state`/`scheduler`/…):
//! db/parser/vector/plugin/vault/redact — листья; chunker→parser; net→plugin,redact,vault; ai→net.

/// AI-слой: раздельные Chat/Embedding провайдеры (ADR-005).
pub mod ai;
/// Markdown-чанкер для RAG (§6.1).
pub mod chunker;
/// БД-слой: rusqlite + write-actor + read-pool (WAL) + миграции схемы (ADR-003).
pub mod db;
/// Egress-граница ядра (ADR-005-ext): `GuardedClient` + политика + audit — единый chokepoint HTTP.
pub mod net;
/// Markdown-парсер (frontmatter, ссылки, теги).
pub mod parser;
/// Plugin loader (минимум): manifest + совместимость версии API (без broker — Ф2).
pub mod plugin;
/// `Redacted<T>`: безопасные Debug/Display (контент/пути не утекают в логи по неосторожности) — AC-SEC-6.
pub mod redact;
/// Vault: ленивый листинг + канонизация путей (анти-traversal).
pub mod vault;
/// Векторный ANN-индекс (usearch HNSW) — §6.1/§6.2.
pub mod vector;
