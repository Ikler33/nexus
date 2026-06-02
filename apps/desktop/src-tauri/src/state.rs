//! Глобальное состояние приложения (Tauri managed state).

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::ai::EmbeddingProvider;
use crate::db::Database;
use crate::vector::VectorIndex;

/// Состояние приложения: текущий открытый vault (или его отсутствие).
pub struct AppState {
    /// `None`, пока vault не открыт; `RwLock` — много читателей команд, редкая смена.
    pub vault: RwLock<Option<VaultContext>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            vault: RwLock::new(None),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Контекст открытого vault: корень на диске + его БД + (опц.) RAG-подсистема.
pub struct VaultContext {
    pub root: PathBuf,
    pub db: Database,
    /// Векторный ANN-индекс RAG. `None`, если embedding-провайдер не сконфигурирован
    /// (vault работает и без AI — local-first). Делится с индексатором (пишет) и поиском (читает).
    pub vectors: Option<Arc<VectorIndex>>,
    /// Embedding-провайдер — для эмбеддинга поисковых запросов (Ф1-6) и чат-RAG (Ф1-8).
    /// `None` синхронно с `vectors` (оба есть или обоих нет).
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
}
