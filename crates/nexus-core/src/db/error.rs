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

    /// Ошибка смежного слоя, всплывшая через индексатор (эмбеддер/векторный индекс).
    /// Держит БД-слой развязанным от `ai`/`vector` (без циклических `From`).
    #[error("{0}")]
    External(String),
}

/// Краткий alias результата БД-операций.
pub type DbResult<T> = Result<T, DbError>;
