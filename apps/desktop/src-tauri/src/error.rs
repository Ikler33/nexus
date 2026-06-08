//! Единый тип ошибки командного слоя (кросс-план #9).
//!
//! Раньше каждая IPC-команда возвращала `Result<T, String>` и вручную звала `.map_err(|e|
//! e.to_string())` на каждом шаге (≈100 мест) — доменные ошибки (`DbError`, `AiError`, …) теряли тип
//! сразу на границе, а одинаковый boilerplate расползался по 14 модулям. [`AppError`] собирает их в
//! один тип: доменные ошибки поднимаются через `?` (`#[from]`), а ad-hoc случаи — [`AppError::Msg`].
//!
//! **Контракт фронта не меняется.** Tauri сериализует ошибку команды и отдаёт её в JS как
//! reject-значение; здесь [`AppError`] сериализуется в **строку** (`Display`), поэтому `tauri-api.ts`
//! и сторы продолжают видеть `string` — миграция чисто внутренняя (Rust-сторона).

use serde::{Serialize, Serializer};

/// Ошибка командного слоя: обёртка над доменными ошибками подсистем + ad-hoc сообщения.
///
/// Команды возвращают [`AppResult<T>`] и пользуются `?` вместо ручного `map_err`. Где нужен
/// контекст («не удалось X: {e}»), оставляем явный `.map_err(|e| AppError::Msg(format!(...)))`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Операция требует открытого vault, а его нет (заменяет прежнее `"vault не открыт".into()`).
    #[error("vault не открыт")]
    NoVault,

    /// Ошибка БД-слоя (rusqlite / миграции / write-actor).
    #[error(transparent)]
    Db(#[from] crate::db::DbError),

    /// Ошибка AI-слоя (chat/embedding провайдеры, HTTP, парсинг ответа).
    #[error(transparent)]
    Ai(#[from] crate::ai::AiError),

    /// Ошибка vault-слоя (канонизация путей, чтение/запись заметок, анти-traversal).
    #[error(transparent)]
    Vault(#[from] crate::vault::VaultError),

    /// Ошибка векторного ANN-индекса (usearch).
    #[error(transparent)]
    Vector(#[from] crate::vector::VectorError),

    /// Ошибка git-sync (libgit2 / remote).
    #[error(transparent)]
    Git(#[from] crate::git::GitError),

    /// Ошибка git-кредов в keychain ОС (Ф3-3b).
    #[error(transparent)]
    Cred(#[from] crate::git::creds::CredError),

    /// Ошибка плагинной подсистемы (loader / broker / capability).
    #[error(transparent)]
    Plugin(#[from] crate::plugin::PluginError),

    /// Файловая ошибка вне vault-слоя (конфиг, служебные файлы).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Прочее, не покрытое доменным типом — ad-hoc текст (бывшие `String`-ошибки команд).
    #[error("{0}")]
    Msg(String),
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::Msg(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::Msg(s.to_string())
    }
}

/// В JS ошибка уходит **строкой** (как и при прежнем `Result<T, String>`) — контракт фронта сохранён.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Краткий alias результата командного слоя.
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_vault_has_stable_message() {
        // UI/тесты могут опираться на текст — фиксируем.
        assert_eq!(AppError::NoVault.to_string(), "vault не открыт");
    }

    #[test]
    fn from_str_and_string_make_msg() {
        assert!(matches!(AppError::from("боом"), AppError::Msg(s) if s == "боом"));
        assert!(matches!(AppError::from("боом".to_string()), AppError::Msg(s) if s == "боом"));
    }

    #[test]
    fn domain_error_lifts_via_question_mark() {
        // `?` на доменной ошибке должен давать соответствующий вариант (через `#[from]`).
        fn inner() -> Result<(), crate::db::DbError> {
            Err(crate::db::DbError::Unavailable)
        }
        fn outer() -> AppResult<()> {
            inner()?;
            Ok(())
        }
        assert!(matches!(outer(), Err(AppError::Db(_))));
    }

    #[test]
    fn serializes_to_plain_string() {
        // Контракт фронта: ошибка = строка, а не объект.
        let json = serde_json::to_string(&AppError::Msg("упс".into())).unwrap();
        assert_eq!(json, "\"упс\"");
    }
}
