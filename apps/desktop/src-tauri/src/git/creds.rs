//! git-credentials в системном keychain (Ф3-3b, AC-SEC-3): токен доступа к remote хранится в keychain
//! ОС (macOS Keychain / Windows Credential Manager / Linux Secret Service), а **НЕ на диске** и не в
//! git. Запись: `service = "nexus-git"`, `account = <идентификатор vault>` (разные vault → разные токены).
//! Используется credentials-callback'ом git2 в pull/push (Ф3-3b-2).

use keyring::Entry;

const SERVICE: &str = "nexus-git";

#[derive(Debug, thiserror::Error)]
pub enum CredError {
    #[error("keychain: {0}")]
    Keyring(#[from] keyring::Error),
}

pub type CredResult<T> = Result<T, CredError>;

fn entry(account: &str) -> CredResult<Entry> {
    Ok(Entry::new(SERVICE, account)?)
}

/// Сохранить токен доступа к remote для vault `account` в keychain ОС.
pub fn set_token(account: &str, token: &str) -> CredResult<()> {
    entry(account)?.set_password(token)?;
    Ok(())
}

/// Получить токен (если есть). `None`, если записи нет.
pub fn get_token(account: &str) -> CredResult<Option<String>> {
    match entry(account)?.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Удалить токен. Отсутствие записи — не ошибка (идемпотентно).
pub fn delete_token(account: &str) -> CredResult<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Есть ли сохранённый токен для vault `account`.
pub fn has_token(account: &str) -> CredResult<bool> {
    Ok(get_token(account)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Роундтрип set→get→has→delete. `#[ignore]`: пишет в РЕАЛЬНЫЙ keychain ОС (на CI без secret
    /// service упадёт) — гонять вручную: `cargo test git::creds -- --ignored`.
    #[test]
    #[ignore = "пишет в реальный keychain ОС"]
    fn token_roundtrip() {
        let acc = "nexus-test-roundtrip-acc";
        set_token(acc, "secret-tok").unwrap();
        assert_eq!(get_token(acc).unwrap().as_deref(), Some("secret-tok"));
        assert!(has_token(acc).unwrap());
        delete_token(acc).unwrap();
        assert_eq!(get_token(acc).unwrap(), None);
        assert!(!has_token(acc).unwrap());
        delete_token(acc).unwrap(); // идемпотентно
    }
}
