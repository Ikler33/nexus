use thiserror::Error;

/// Ошибки БД-слоя Nexus.
#[derive(Debug, Error)]
pub enum DbError {
    /// Ошибка SQLite / rusqlite.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Ошибка файловой системы при открытии БД (создание `.nexus/` и т.п.).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Сбой применения миграции схемы (с указанием версии).
    #[error("миграция v{version}: {source}")]
    Migration {
        version: u32,
        #[source]
        source: rusqlite::Error,
    },

    /// Поток-писатель или коннект недоступен (канал закрыт / задача прервана).
    #[error("БД недоступна: поток-писатель или коннект завершён")]
    Unavailable,
}

/// Краткий alias результата БД-операций.
pub type DbResult<T> = Result<T, DbError>;
