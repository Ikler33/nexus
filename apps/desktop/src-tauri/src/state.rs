//! Глобальное состояние приложения (Tauri managed state).

use std::path::PathBuf;

use tokio::sync::RwLock;

use crate::db::Database;

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

/// Контекст открытого vault: корень на диске + его БД.
pub struct VaultContext {
    pub root: PathBuf,
    pub db: Database,
}
