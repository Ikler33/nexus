//! Глобальное состояние приложения (Tauri managed state).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::sync::RwLock;

use crate::ai::{ChatProvider, EmbeddingProvider};
use crate::db::Database;
use crate::vector::VectorIndex;

/// Состояние приложения: текущий открытый vault (или его отсутствие).
pub struct AppState {
    /// `None`, пока vault не открыт; `RwLock` — много читателей команд, редкая смена.
    pub vault: RwLock<Option<VaultContext>>,
    /// Флаг отмены активного чат-стрима (UI ведёт один чат за раз). `chat_rag` ставит новый
    /// токен (отменяя предыдущий), `chat_cancel` его взводит. `std::Mutex` — держим коротко, без await.
    pub chat_cancel: Mutex<Option<Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            vault: RwLock::new(None),
            chat_cancel: Mutex::new(None),
        }
    }

    /// Взводит флаг отмены текущего чат-стрима (если есть).
    pub fn cancel_active_chat(&self) {
        if let Ok(guard) = self.chat_cancel.lock() {
            if let Some(flag) = guard.as_ref() {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// Регистрирует новый токен отмены для начинающегося чат-стрима, отменив предыдущий.
    pub fn begin_chat(&self) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        if let Ok(mut guard) = self.chat_cancel.lock() {
            if let Some(prev) = guard.replace(token.clone()) {
                prev.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        token
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
    /// Chat-провайдер (ADR-005, отдельный хост) — стриминг ответов RAG-чата (Ф1-7).
    /// `None`, если в `local.json` нет `ai.chat`. Независим от embedder.
    pub chat: Option<Arc<dyn ChatProvider>>,
}
