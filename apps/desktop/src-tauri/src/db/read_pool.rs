use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tokio::sync::Semaphore;

use super::error::{DbError, DbResult};

/// Пул read-коннектов (**ADR-003**).
///
/// WAL допускает параллельное чтение одновременно с единственным писателем. Число
/// одновременных читателей ограничено семафором; rusqlite синхронен, поэтому каждый
/// запрос выполняется в `spawn_blocking` и не блокирует async-рантайм.
#[derive(Clone)]
pub struct ReadPool {
    inner: Arc<Inner>,
}

struct Inner {
    /// Свободные коннекты. Инвариант: число доступных permit'ов == числу коннектов
    /// в этом векторе в момент, когда permit можно получить.
    conns: Mutex<Vec<Connection>>,
    permits: Semaphore,
}

impl ReadPool {
    pub(crate) fn new(conns: Vec<Connection>) -> Self {
        let n = conns.len();
        Self {
            inner: Arc::new(Inner {
                conns: Mutex::new(conns),
                permits: Semaphore::new(n),
            }),
        }
    }

    /// Выполняет read-запрос `f` на свободном коннекте из пула.
    pub async fn query<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        // Permit гарантирует наличие коннекта в пуле (permits == conns).
        let _permit = self
            .inner
            .permits
            .acquire()
            .await
            .map_err(|_| DbError::Unavailable)?;
        let conn = {
            let mut guard = self.inner.conns.lock().expect("read pool mutex poisoned");
            guard
                .pop()
                .expect("read-pool invariant: connection available per permit")
        };

        // rusqlite синхронен → уводим в blocking-пул; коннект возвращаем в пул после.
        let (conn, result) = tokio::task::spawn_blocking(move || {
            let r = f(&conn);
            (conn, r)
        })
        .await
        .map_err(|_| DbError::Unavailable)?;

        self.inner
            .conns
            .lock()
            .expect("read pool mutex poisoned")
            .push(conn);
        result.map_err(DbError::from)
        // _permit освобождается здесь — уже ПОСЛЕ возврата коннекта в пул.
    }
}
