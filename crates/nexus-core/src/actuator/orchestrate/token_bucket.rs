//! Токен-бакет анти-усталости (AGENT-5) + тестируемый источник времени [`Clock`].
//!
//! Самодостаточная rate-limiting-единица гейта автономии, вынесенная из `orchestrate.rs` (R-5b,
//! чистый перенос без изменения логики): [`TokenBucket`] (claim-before-apply, ленивый рефилл,
//! refund) и [`Clock`]-шов ([`MonotonicClock`] в проде, [`ManualClock`] для детерминированных
//! тестов времени). Публичные имена реэкспортируются `orchestrate` без изменения путей
//! (`orchestrate::TokenBucket` и т.д.).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Источник монотонного времени для [`TokenBucket`] — ТЕСТИРУЕМЫЙ ШОВ (AGENT-5). Прод-реализация
/// [`MonotonicClock`] читает `Instant`; тесты подставляют [`ManualClock`] с РУЧНЫМ продвижением, поэтому
/// рефилл проверяется БЕЗ `Instant::now()`/`sleep` (детерминированно). Возвращает прошедшее время от
/// эпохи бакета как [`Duration`] — этого достаточно для арифметики рефилла (нам не нужен абсолютный
/// момент, только МОНОТОННАЯ разница).
pub trait Clock: Send + Sync {
    /// Монотонно неубывающее время, прошедшее с эпохи бакета.
    fn now(&self) -> Duration;
}

/// Прод-часы: прошедшее от `Instant` создания. Монотонны (не прыгают назад при смене системного времени).
pub struct MonotonicClock {
    epoch: Instant,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }
}

/// Ручные часы для ДЕТЕРМИНИРОВАННЫХ тестов рефилла: логическое время в наносекундах за `AtomicU64`,
/// продвигается только `advance()`. Никаких `Instant::now()`/`sleep` в тестах токен-бакета.
#[cfg(any(test, feature = "test-util"))]
pub struct ManualClock {
    nanos: AtomicU64,
}

#[cfg(any(test, feature = "test-util"))]
impl ManualClock {
    /// Новые часы на нуле.
    pub fn new() -> Self {
        Self {
            nanos: AtomicU64::new(0),
        }
    }
    /// Продвинуть логическое время на `d` (имитация прошедшего времени для рефилла).
    pub fn advance(&self, d: Duration) {
        self.nanos.fetch_add(d.as_nanos() as u64, Ordering::SeqCst);
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::SeqCst))
    }
}

/// **АНТИ-УСТАЛОСТЬ ТОКЕН-БАКЕТ (AGENT-5)** — заменяет 3d-простой кэп: вместит до `capacity`
/// авто-применений Auto-тира В ПАЧКЕ, затем РЕЙТ-ЛИМИТИТ (рефилл `refill_tokens` каждые `refill_per`),
/// форсируя предложение, пока токены не восполнятся. Делится за `Arc` между диспетчами/инструментами
/// одного прогона (анти-усталость кросс-инструментна).
///
/// # CLAIM-BEFORE-APPLY (concurrency-safe)
/// 3d делал `under_cap()` → apply → `bump()` — это check-then-act ГОНКА: два конкурентных диспетча
/// могли оба прочитать `count<cap`, оба применить и ПРЕВЫСИТЬ кэп. [`TokenBucket::try_claim`] КЛЕЙМИТ
/// токен АТОМАРНО (`compare_exchange` на счётчике available) ДО apply: успех ⇒ токен ЗАБРОНИРОВАН,
/// применяем; провал (бакет пуст) ⇒ предлагаем. N конкурентных claim'ов на ёмкости N дают РОВНО N
/// успехов (CAS сериализует декремент) — превысить ёмкость нечем.
///
/// # НЕ Applied ⇒ РЕФАНД (токен не тратится)
/// Токен клеймится ДО apply (иначе гонка), но apply может вернуть НЕ-Applied (Failed: drift/запись).
/// Тогда [`TokenBucket::refund`] возвращает токен в бакет (не выше capacity) — «потрачен» лишь реально
/// применённый Auto. (Concurrency-safe: refund — атомарный CAS-инкремент с потолком capacity.)
///
/// # Время — через [`Clock`]-шов
/// Рефилл считается от `clock.now()` (прод: `Instant`; тест: [`ManualClock`]). Рефилл ЛЕНИВЫЙ: при
/// каждом claim сперва доначисляем токены за прошедшие полные окна `refill_per`, продвигая `last_refill`
/// CAS'ом (только один поток применяет данное окно — без двойного начисления при гонке).
#[derive(Clone)]
pub struct TokenBucket {
    /// Доступные токены (клеймятся CAS-декрементом; рефилл — CAS-инкремент до capacity).
    available: Arc<AtomicU32>,
    /// Момент (нанос от эпохи Clock) последнего применённого рефилл-окна. Продвигается CAS'ом.
    last_refill_nanos: Arc<AtomicU64>,
    /// Ёмкость бакета (потолок токенов; макс. размер пачки). Маппится из `blast_radius_cap` конфига.
    capacity: u32,
    /// Сколько токенов доначислять за каждое прошедшее окно `refill_per`.
    refill_tokens: u32,
    /// Длительность окна рефилла. Ноль ⇒ рефилла нет (чистый кумулятивный кэп — поведение 3d-кэпа).
    refill_per: Duration,
    /// Источник времени (прод/тест-шов).
    clock: Arc<dyn Clock>,
}

impl TokenBucket {
    /// Бакет ёмкости `capacity`, рефилл `refill_tokens` за `refill_per`, на прод-часах. Стартует ПОЛНЫМ
    /// (capacity токенов) — прогон сразу может применить пачку. `refill_per == 0` ⇒ рефилла нет (бакет
    /// исчерпывается и остаётся пустым — кумулятивный кэп, как 3d-`blast_radius`).
    pub fn new(capacity: u32, refill_tokens: u32, refill_per: Duration) -> Self {
        Self::with_clock(
            capacity,
            refill_tokens,
            refill_per,
            Arc::new(MonotonicClock::default()),
        )
    }

    /// Как [`TokenBucket::new`], но с ИНЪЕКТИРОВАННЫМИ часами (тестовый шов). Стартует полным.
    pub fn with_clock(
        capacity: u32,
        refill_tokens: u32,
        refill_per: Duration,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            available: Arc::new(AtomicU32::new(capacity)),
            last_refill_nanos: Arc::new(AtomicU64::new(clock.now().as_nanos() as u64)),
            capacity,
            refill_tokens,
            refill_per,
            clock,
        }
    }

    /// Текущее число доступных токенов (после ленивого рефилла). Диагностика/тесты.
    pub fn available(&self) -> u32 {
        self.refill_now();
        self.available.load(Ordering::SeqCst)
    }

    /// Ёмкость бакета (потолок).
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Ленивый рефилл: доначислить токены за все ПОЛНЫЕ окна `refill_per`, прошедшие с `last_refill`.
    /// `refill_per == 0` или `refill_tokens == 0` ⇒ no-op. `last_refill` продвигается CAS'ом ровно на
    /// `windows * refill_per` (НЕ на текущий момент) — остаток времени окна не теряется, и только один
    /// поток применяет данный набор окон (конкурент увидит уже-продвинутый `last_refill`).
    fn refill_now(&self) {
        if self.refill_per.is_zero() || self.refill_tokens == 0 {
            return;
        }
        let per = self.refill_per.as_nanos() as u64;
        let now = self.clock.now().as_nanos() as u64;
        loop {
            let last = self.last_refill_nanos.load(Ordering::SeqCst);
            if now <= last {
                return;
            }
            let elapsed = now - last;
            let windows = elapsed / per;
            if windows == 0 {
                return;
            }
            let advance = windows * per;
            // Бронируем окна: продвигаем last_refill атомарно. Проигравший CAS — повторит (увидит
            // новый last). Победитель доначисляет токены за СВОИ `windows`.
            if self
                .last_refill_nanos
                .compare_exchange(last, last + advance, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                continue;
            }
            // Доначислить windows*refill_tokens, не превышая capacity (CAS — конкурентно-безопасно).
            let add = (windows as u32)
                .saturating_mul(self.refill_tokens)
                .min(self.capacity);
            self.add_capped(add);
            return;
        }
    }

    /// Атомарно прибавить `add` токенов, НЕ превышая capacity (CAS-цикл; конкурентно-безопасно).
    fn add_capped(&self, add: u32) {
        if add == 0 {
            return;
        }
        let mut cur = self.available.load(Ordering::SeqCst);
        loop {
            let next = cur.saturating_add(add).min(self.capacity);
            if next == cur {
                return;
            }
            match self.available.compare_exchange_weak(
                cur,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(actual) => cur = actual,
            }
        }
    }

    /// **CLAIM-BEFORE-APPLY**: атомарно забронировать ОДИН токен. `true` ⇒ токен снят (можно применять),
    /// `false` ⇒ бакет пуст (форсировать предложение). Сперва ленивый рефилл, затем CAS-декремент: при
    /// гонке два потока НЕ могут снять один и тот же токен (CAS сериализует), поэтому суммарно успешных
    /// claim'ов ≤ доступных токенов — ПРЕВЫСИТЬ ёмкость НЕЛЬЗЯ.
    pub fn try_claim(&self) -> bool {
        self.refill_now();
        let mut cur = self.available.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return false;
            }
            match self.available.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Вернуть забронированный токен (apply вернул НЕ-Applied: Failed). Не выше capacity — конкурентно
    /// безопасно (атомарный CAS-инкремент с потолком).
    pub fn refund(&self) {
        self.add_capped(1);
    }
}

/// Окно рефилла токен-бакета по умолчанию (AGENT-5): один токен восстанавливается каждые 60 с. Вместе
/// с дефолтной ёмкостью (`blast_radius_cap` конфига) даёт «пачка до N, затем ~1/мин» — щадящий рейт,
/// чтобы автономный агент не «уставлял» владельца лавиной авто-правок, но и не вставал намертво в
/// длинном легитимном прогоне. Маппинг конфига: capacity = `blast_radius_cap`, refill = 1 / этот период.
pub const DEFAULT_REFILL_PER: Duration = Duration::from_secs(60);
/// Сколько токенов доначислять за окно [`DEFAULT_REFILL_PER`] (один — плавный рейт-лимит).
pub const DEFAULT_REFILL_TOKENS: u32 = 1;
