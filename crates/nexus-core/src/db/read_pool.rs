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

        // rusqlite синхронен → уводим в blocking-пул; коннект возвращаем в пул после. Паника `f`
        // ловится ВНУТРИ blocking-таска (catch_unwind), чтобы коннект вернулся в пул в ЛЮБОМ случае:
        // иначе панический read терял коннект (permit освобождался, а conn — нет → инвариант
        // permits==conns ломался, и следующий pop паниковал на exhausted-пуле). Находка аудита.
        let (conn, result) = tokio::task::spawn_blocking(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&conn)));
            (conn, r)
        })
        .await
        .map_err(|_| DbError::Unavailable)?;

        self.inner
            .conns
            .lock()
            .expect("read pool mutex poisoned")
            .push(conn);
        match result {
            Ok(r) => r.map_err(DbError::from),
            Err(_) => {
                tracing::error!("read-pool: read-замыкание паниковало — коннект возвращён в пул");
                Err(DbError::Unavailable)
            }
        }
        // _permit освобождается здесь — уже ПОСЛЕ возврата коннекта в пул.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Аудит: паника read-замыкания не теряет коннект — он возвращается в пул (catch_unwind в blocking),
    /// инвариант permits==conns цел; следующий запрос работает (иначе `pop().expect` паниковал бы на
    /// опустошённом пуле). В release(panic=abort) неприменимо, но `cargo test` = panic=unwind.
    #[tokio::test]
    async fn read_pool_survives_panicking_query() {
        let pool = ReadPool::new(vec![Connection::open_in_memory().unwrap()]); // пул из 1 коннекта
        let panicked = pool
            .query(|_c| -> rusqlite::Result<i64> { panic!("boom") })
            .await;
        assert!(panicked.is_err(), "паническое чтение → ошибка");
        // Коннект вернулся — единственный в пуле; следующий запрос не паникует на пустом пуле.
        let ok = pool
            .query(|c| c.query_row("SELECT 7", [], |r| r.get::<_, i64>(0)))
            .await;
        assert_eq!(ok.unwrap(), 7, "пул пережил панику, коннект возвращён");
    }
}
