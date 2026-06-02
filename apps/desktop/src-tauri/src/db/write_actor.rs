use std::thread;

use rusqlite::{Connection, Transaction};
use tokio::sync::{mpsc, oneshot};

use super::error::{DbError, DbResult};

/// Задание для потока-писателя: замыкание над единственным write-коннектом.
type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Единственный писатель БД (**ADR-003**).
///
/// Все мутации сериализуются через один поток с синхронными транзакциями rusqlite.
/// Это исключает `SQLITE_BUSY` между писателями (AC-Б7-1) и делает невозможной гонку
/// двух write-транзакций. Клонируется дёшево (общий канал) для передачи в indexer и
/// Tauri-команды.
#[derive(Clone)]
pub struct WriteActor {
    tx: mpsc::UnboundedSender<Job>,
}

impl WriteActor {
    /// Поднимает поток-писатель, забирая владение уже сконфигурированным коннектом
    /// (WAL/pragmas/миграции применяются до вызова). Поток завершается, когда закрыт
    /// последний клон отправителя; коннект при этом закрывается (checkpoint WAL).
    pub(crate) fn spawn(mut conn: Connection) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        thread::Builder::new()
            .name("nexus-db-writer".into())
            .spawn(move || {
                while let Some(job) = rx.blocking_recv() {
                    job(&mut conn);
                }
            })
            .expect("failed to spawn nexus-db-writer thread");
        Self { tx }
    }

    /// Выполняет произвольную операцию на write-коннекте (без авто-транзакции).
    pub async fn call<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (res_tx, res_rx) = oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let _ = res_tx.send(f(conn));
        });
        self.tx.send(job).map_err(|_| DbError::Unavailable)?;
        res_rx
            .await
            .map_err(|_| DbError::Unavailable)?
            .map_err(DbError::from)
    }

    /// Выполняет `f` внутри ОДНОЙ синхронной транзакции: commit при `Ok`, полный
    /// rollback при `Err` или панике (атомарность индексации — AC-Б7-2).
    pub async fn transaction<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&Transaction) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let out = f(&tx)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }
}
