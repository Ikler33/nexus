//! R-9 — [`RpcCorrelator`]: КАНОН корреляции исходящих JSON-RPC-запросов по числовому `id`. Дедуп ×3→1
//! утроенной ранее логики (Pending-map + монотонный счётчик id + drain-on-close + timeout-обёртка):
//! [`super::super::agent::connect::ConnectClient`] (CONN-2), [`super::super::agent::connect::acp`]
//! `AcpClient` (ACP-1, задокументированный TODO «rpc-core продублирован») и `AcpServerDecisionSource`
//! (ACP-2 perm_pending — третья копия).
//!
//! # Разделение обязанностей
//! Коррелятор знает ТОЛЬКО про `id`↔ждущий-oneshot: он НЕ трогает транспорт и НЕ знает методов протокола.
//! - Отправка запроса на провод и КОНКРЕТНОЕ значение таймаута — на стороне потребителя ([`begin`] →
//!   `transport.send` → [`await_reply`]). Таймауты у трёх потребителей РАЗНЫЕ (30с управляющих RPC клиента /
//!   `Option` у ACP-клиента, `None`=`session/prompt` без лимита / 300с ожидания permission у ACP-сервера) —
//!   поэтому таймаут приходит ПАРАМЕТРОМ в [`await_reply`], а НЕ зашит в коррелятор.
//! - Роутинг входящих ответов по `id` остаётся у read-loop'а потребителя ([`resolve`]).
//! - Инвариант закрытия соединения (все ждущие → провалить/резолвить, R1-R4 из CONN-2) — [`fail_all`].
//!
//! Обобщён по типу доставляемого значения `T` (прямой generic, без trait-магии): три потребителя после
//! R-1 шлют один и тот же `Result<Value, RpcError>`, но коррелятор о протоколе не знает и остаётся честно
//! параметризованным — новый потребитель с иным payload'ом подключается без правок канона.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use tokio::sync::{oneshot, Mutex};

/// Коррелятор запрос→ответ по числовому `id`: карта ждущих `oneshot::Sender<T>` + монотонный счётчик id.
/// Один экземпляр на соединение/направление; безопасен для конкурентных ждущих (≥2 in-flight на один
/// коррелятор) — каждый запрос держит собственный oneshot, карта разводит их по уникальному `id`.
pub struct RpcCorrelator<T> {
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<T>>>,
}

impl<T> RpcCorrelator<T> {
    /// Новый коррелятор со счётчиком id, стартующим с `id_base`: клиентские направления — `1`; ACP-сервер
    /// permission — `PERM_ID_BASE` (большой оффсет, чтобы наши исходящие id НИКОГДА не пересеклись с id
    /// клиента — belt-and-suspenders, направления и так раздельны).
    pub fn new(id_base: i64) -> Self {
        Self {
            next_id: AtomicI64::new(id_base),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Регистрирует новый исходящий запрос: аллоцирует `id`, кладёт `oneshot::Sender` в карту, отдаёт
    /// `(id, rx)`. Вызывать ДО отправки запроса на провод — ответ не может опередить регистрацию.
    pub async fn begin(&self) -> (i64, oneshot::Receiver<T>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        (id, rx)
    }

    /// Роутит ответ ждущему по `id` (задача read-loop'а). Снимает запись ДО доставки (не двоит). Неизвестный
    /// `id` (поздний/дубль/чужой) — тихий no-op.
    pub async fn resolve(&self, id: i64, value: T) {
        if let Some(tx) = self.pending.lock().await.remove(&id) {
            let _ = tx.send(value);
        }
    }

    /// Снимает ожидание по `id` без доставки — cleanup при send-fail / таймауте / закрытом oneshot.
    /// Идемпотентно: штатный [`resolve`]/[`fail_all`] мог уже снять запись (гонка timeout↔resolve безопасна).
    pub async fn cancel(&self, id: i64) {
        self.pending.lock().await.remove(&id);
    }

    /// Дренирует ВСЕ ждущие, доставляя каждому `value` (drain-on-close при EOF/ошибке транспорта, а также
    /// массовая отмена). Карта очищается — «не теряет/не двоит»: каждый oneshot получает значение РОВНО раз.
    pub async fn fail_all(&self, value: T)
    where
        T: Clone,
    {
        let mut pending = self.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(value.clone());
        }
    }

    /// Ждёт ответ по `(id, rx)` с ОПЦИОНАЛЬНЫМ таймаутом (значение таймаута — параметр потребителя, НЕ
    /// унифицируется). Доставленное значение возвращается как есть; закрытый oneshot → `on_closed()`,
    /// истёкший таймаут → `on_timeout()`. В обоих fallback-путях снимает `id` из карты (идемпотентно —
    /// гонка с [`resolve`] безопасна: oneshot доставляет РОВНО раз).
    pub async fn await_reply(
        &self,
        id: i64,
        rx: oneshot::Receiver<T>,
        timeout: Option<Duration>,
        on_closed: impl FnOnce() -> T,
        on_timeout: impl FnOnce() -> T,
    ) -> T {
        let delivered = match timeout {
            Some(d) => match tokio::time::timeout(d, rx).await {
                Ok(delivered) => delivered,
                Err(_) => {
                    self.cancel(id).await;
                    return on_timeout();
                }
            },
            None => rx.await,
        };
        match delivered {
            Ok(value) => value,
            Err(_) => {
                self.cancel(id).await;
                on_closed()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Payload юнитов — простой i32 (коррелятор о протоколе не знает; generic честно параметризуется).
    fn corr() -> RpcCorrelator<i32> {
        RpcCorrelator::new(1)
    }

    /// begin выдаёт монотонные id от базы; resolve по id доставляет значение ждущему.
    #[tokio::test]
    async fn begin_monotonic_and_resolve_routes() {
        let c = RpcCorrelator::<i32>::new(1_000_000_000);
        let (id1, rx1) = c.begin().await;
        let (id2, rx2) = c.begin().await;
        assert_eq!(id1, 1_000_000_000);
        assert_eq!(id2, 1_000_000_001, "счётчик стартует с базы и монотонен");
        // Резолвим ВТОРОЙ раньше первого — out-of-order демукс по id.
        c.resolve(id2, 42).await;
        c.resolve(id1, 7).await;
        assert_eq!(rx2.await.unwrap(), 42);
        assert_eq!(rx1.await.unwrap(), 7);
    }

    /// Два одновременных ждущих на один коррелятор не путаются (конкурентность).
    #[tokio::test]
    async fn concurrent_pending_are_independent() {
        let c = corr();
        let (id_a, rx_a) = c.begin().await;
        let (id_b, rx_b) = c.begin().await;
        assert_ne!(id_a, id_b);
        c.resolve(id_a, 1).await;
        assert_eq!(rx_a.await.unwrap(), 1);
        // b всё ещё висит и разрешается независимо.
        c.resolve(id_b, 2).await;
        assert_eq!(rx_b.await.unwrap(), 2);
    }

    /// resolve неизвестного id — тихий no-op (поздний/дублирующий ответ не паникует и не двоит).
    #[tokio::test]
    async fn resolve_unknown_id_is_noop() {
        let c = corr();
        c.resolve(12345, 99).await; // ничего не зарегистрировано
        let (id, rx) = c.begin().await;
        c.resolve(id, 5).await;
        c.resolve(id, 6).await; // второй резолв того же id — уже снят, no-op
        assert_eq!(rx.await.unwrap(), 5, "доставляется только первый резолв");
    }

    /// fail_all дренирует ВСЕХ ждущих одним значением; карта пустеет (drain-on-close).
    #[tokio::test]
    async fn fail_all_drains_every_pending() {
        let c = corr();
        let (_id1, rx1) = c.begin().await;
        let (_id2, rx2) = c.begin().await;
        c.fail_all(-1).await;
        assert_eq!(rx1.await.unwrap(), -1);
        assert_eq!(rx2.await.unwrap(), -1);
        // После дренажа карта пуста → новый begin переиспользуемо работает, старые id не резолвятся.
        let (id3, rx3) = c.begin().await;
        c.resolve(id3, 3).await;
        assert_eq!(rx3.await.unwrap(), 3);
    }

    /// await_reply: доставленное значение возвращается как есть (таймаут не срабатывает).
    #[tokio::test]
    async fn await_reply_delivers_value() {
        let c = corr();
        let (id, rx) = c.begin().await;
        c.resolve(id, 77).await;
        let v = c
            .await_reply(id, rx, Some(Duration::from_secs(5)), || -1, || -2)
            .await;
        assert_eq!(v, 77);
    }

    /// await_reply: истёкший таймаут → on_timeout() и запись снята из карты.
    #[tokio::test]
    async fn await_reply_timeout_cleans_up() {
        let c = corr();
        let (id, rx) = c.begin().await;
        let v = c
            .await_reply(id, rx, Some(Duration::from_millis(20)), || -1, || -2)
            .await;
        assert_eq!(v, -2, "on_timeout()");
        // Запись снята: поздний resolve — no-op (не паникует).
        c.resolve(id, 5).await;
    }

    /// await_reply: закрытый oneshot (sender дропнут без send) → on_closed().
    #[tokio::test]
    async fn await_reply_closed_channel() {
        let c = corr();
        let (id, rx) = c.begin().await;
        // Снимаем sender из карты и дропаем его без доставки → rx видит закрытие.
        {
            let mut p = c.pending.lock().await;
            drop(p.remove(&id));
        }
        let v = c
            .await_reply(id, rx, Some(Duration::from_secs(5)), || -1, || -2)
            .await;
        assert_eq!(v, -1, "on_closed()");
    }

    /// await_reply без таймаута (None) ждёт доставку (ветка session/prompt).
    #[tokio::test]
    async fn await_reply_no_timeout_waits() {
        let c = std::sync::Arc::new(corr());
        let (id, rx) = c.begin().await;
        let c2 = c.clone();
        let waiter = tokio::spawn(async move { c2.await_reply(id, rx, None, || -1, || -2).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        c.resolve(id, 55).await;
        assert_eq!(waiter.await.unwrap(), 55);
    }
}
