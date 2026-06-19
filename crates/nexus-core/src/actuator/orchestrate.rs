//! Гейт автономии актуатора (AGENT-3d) — РЕШАЕТ, применить действие СРАЗУ или ПРЕДЛОЖИТЬ и ждать решения.
//!
//! [`dispatch_action`] — диспетч `(RiskTier × autonomy)`, заменяющий 3c-стаб Confirm-шва. Берёт типизи-
//! рованное [`Action`], `run_id`, автономию прогона (`confirm`|`auto`, `None`=confirm = безопаснее),
//! [`DecisionSource`], [`EventSink`] (эмиссия Proposal/Diff — 3e свяжет с `on_event` цикла; здесь —
//! коллектор/канал теста), ledger, канон-корень, blast-radius-состояние и `overwrite_threshold` ИЗ
//! КОНФИГА (не 64KiB-константу). Поток:
//!  1. читаем on-disk содержимое цели → `classify_hash` (токен оптимистичной конкуренции на момент
//!     classify) + база для диффа;
//!  2. [`classify`] с `ctx.overwrite_threshold` из конфига (3c hard-gate: порог — параметр, не хардкод);
//!  3. матч `(тир, автономия)` — см. [`Dispatch`]-матрицу ниже;
//!  4. на «предложить»: ledger-строка `proposed` → эмиссия Proposal+Diff → [`DecisionSource::decide`] →
//!     Approve ⇒ `proposed→approved` ⇒ [`apply_action`] (с ОБЯЗАТЕЛЬНЫМ `classify_hash`); Reject ⇒
//!     `proposed→rejected` (finish с исходом), БЕЗ записи.
//!
//! ## Матрица `(RiskTier × autonomy)` — keystone безопасности 3d
//! | тир \ автономия | `auto`-прогон                                   | `confirm`-прогон / `None` |
//! |-----------------|------------------------------------------------|---------------------------|
//! | **HardBlocked** | ToolError::Exec (ВСЕГДА — апрув не разблокирует)| ToolError::Exec           |
//! | **Auto**        | apply СРАЗУ (если blast-radius под кэпом); ИНАЧЕ — предложить (анти-усталость) | предложить + ждать решения |
//! | **Confirm**     | **предложить + ждать** (auto НЕ перекрывает Confirm!) | предложить + ждать         |
//!
//! Два инварианта, которые нельзя нарушить:
//!  - **auto НЕ перекрывает Confirm-тир**: действие, классифицированное Confirm (напр. крупная пере-
//!    запись), ВСЕГДА предлагается, даже в auto-прогоне. auto ускоряет только Auto-тир.
//!  - **blast-radius кэп → предложить**: в auto-прогоне Auto-тир авто-применяется лишь пока кумулятивное
//!    число авто-применений прогона под кэпом; за кэпом — форсируем предложение (анти-усталость; полный
//!    token-bucket/TTL — AGENT-5).
//!
//! ## ОБЯЗАТЕЛЬНЫЙ `classify_hash` на пути apply (3c hard-gate)
//! 3c оставил `classify_hash=None` у инструментов (шов). 3d ОБЯЗАН передавать `Some(classify_hash)` в
//! [`apply_action`] на ЛЮБОМ применяющем пути (Auto-авто и approved-Confirm) — тогда drift-проверка
//! Рубежа 3 (диск изменился между classify и apply) срабатывает ДО снапшота, не только ре-ридом перед
//! записью. Конвенция значения зеркалит apply Рубеж 3 (`on_disk_hash.unwrap_or("")`): для существующего
//! файла — `content_hash(current)`, для отсутствующего (create) — пустая строка `""` (цели нет).
//!
//! ## Граница 3d/3e (НЕ здесь)
//! Регистрации в agentd/реестре и живой проводки НЕТ — гейт + [`DecisionSource`] КОНСТРУИРУЮТСЯ и
//! ТЕСТИРУЮТСЯ здесь (с `ChannelDecision`/моком/коллектором). Реальный vault пользователя не затронут
//! (дисковые записи — только во временных vault'ах тестов). kill-switch / полный token-bucket — AGENT-5.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::agent::event::{AgentEvent, FileStatus, ProposedFile};
use crate::agent::ToolError;

use super::action::{Action, ActionTarget};
use super::apply::{apply_action, ApplyOutcome, AuditSink};
use super::audit::{
    self, canonical_args, idempotency_key, ActionEntry, STATE_APPROVED, STATE_PROPOSED,
    STATE_REJECTED,
};
use super::classify::{classify, BlockReason, ClassifyCtx, RiskTier};
use super::decision::{DecisionSource, ItemDecision, ProposalBatch, ProposalItem};

/// Приёмник [`AgentEvent`] для гейта (эмиссия Proposal/Diff). Object-safe (`&self` + interior mutability
/// у реализаций) — гейт держит `&dyn EventSink` и шлёт события синхронно. 3e свяжет его с `on_event`
/// цикла (адаптер-обёртка над `FnMut`); тесты используют [`CollectingSink`] (копит события в `Vec`).
///
// FIXME(UI-1): связать EventSink.emit → on_event цикла / control-plane-стрим для real-time ревью
// предложений. Сегодня единственная живая реализация на проводке — [`TracingEventSink`] (headless
// agentd): предложения только ЛОГИРУЮТСЯ, не стримятся в UI; под [`PolicyDefault`] они тут же
// auto-DENY-отклоняются (нет интерактивного одобрения). UI-1 добавит человеко-в-петле поверхность
// (стрим Proposal/Diff пользователю + ответ Approve/Reject через DecisionSource).
pub trait EventSink: Send + Sync {
    /// Принять событие хода (Proposal/Diff и т.п.).
    fn emit(&self, event: AgentEvent);
}

/// Тестовый/диагностический сборщик событий — копит эмитированные [`AgentEvent`] в `Vec` за `Mutex`
/// (interior mutability: `emit(&self, …)`). Снять накопленное — [`CollectingSink::events`].
#[derive(Default)]
pub struct CollectingSink {
    events: std::sync::Mutex<Vec<AgentEvent>>,
}

impl CollectingSink {
    /// Новый пустой сборщик.
    pub fn new() -> Self {
        Self::default()
    }

    /// Снимок накопленных событий (в порядке эмиссии).
    pub fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().expect("event mutex").clone()
    }
}

impl EventSink for CollectingSink {
    fn emit(&self, event: AgentEvent) {
        self.events.lock().expect("event mutex").push(event);
    }
}

/// EventSink-мост для HEADLESS agentd (AGENT-3e §4): `tracing`-логирует Proposal/Diff. Долговечная
/// запись changeset'а — это ledger (`agent_actions`); UI-стриминг предложений в `on_event`/AgentEvent
/// поток — это UI-1 (нет UI у headless). Здесь — наблюдаемость: оператор видит в логе, ЧТО гейт
/// предложил. Под [`PolicyDefault`] предложения короткоживущи (тут же auto-DENY-отклоняются), но лог
/// предложения остаётся для аудита. Прочие события игнорируются (цикл шлёт свои через `on_event`).
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingEventSink;

impl TracingEventSink {
    /// Новый sink (бесстейтовый).
    pub fn new() -> Self {
        Self
    }
}

impl EventSink for TracingEventSink {
    fn emit(&self, event: AgentEvent) {
        match event {
            AgentEvent::Proposal { run_id, files } => {
                tracing::info!(
                    run_id,
                    files = files.len(),
                    paths = ?files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
                    "actuator: предложение changeset'а (headless — решает DecisionSource)"
                );
            }
            AgentEvent::Diff {
                path,
                add,
                del,
                status,
            } => {
                tracing::info!(%path, add, del, ?status, "actuator: дифф предложенного файла");
            }
            // Прочие события цикла идут через on_event — здесь не наша забота.
            _ => {}
        }
    }
}

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

/// Политика автономии прогона + параметры гейта. `overwrite_threshold` — ИЗ КОНФИГА (run-policy), НЕ
/// хардкод-константа (3c hard-gate). `token_bucket` — анти-усталость АНТИ-УСТАЛОСТЬ (AGENT-5): ёмкость
/// (из `blast_radius_cap` конфига) + рефилл по времени; за пустым бакетом Auto-тир форсирует предложение.
#[derive(Clone)]
pub struct DispatchPolicy {
    /// `"auto"` ⇒ авто-применение Auto-тира; иначе (`"confirm"`/`None`/прочее) ⇒ предлагать всё.
    pub auto: bool,
    /// Порог «крупной перезаписи» (байт) → Confirm. Источник — конфиг прогона.
    pub overwrite_threshold: usize,
    /// Токен-бакет авто-применений Auto-тира (анти-усталость, AGENT-5). Общий на прогон (делится между
    /// диспетчами/инструментами). Пуст ⇒ Auto форсирует предложение. Claim-before-apply (concurrency-safe).
    pub token_bucket: TokenBucket,
    /// **KILL-SWITCH (AGENT-5, чек-пойнт #3): глобальная пауза агента.** Взведён ⇒ `dispatch_action`
    /// НЕ пишет (форс-предложение вместо авто-apply; отказ применять даже одобренное). Fail-safe.
    /// `DispatchPolicy::new` ставит сюда вечно-НЕвзведённый Arc (поведение без kill-switch); проводка
    /// прогона ([`crate::agent::AgentRunHandler`]) передаёт ОБЩИЙ process-global Arc через `with_paused`.
    pub agent_paused: Arc<AtomicBool>,
}

impl DispatchPolicy {
    /// Собрать политику из автономии прогона (`Some("auto")` ⇒ auto, иначе confirm = безопаснее),
    /// порога перезаписи (конфиг) и ЁМКОСТИ токен-бакета (`blast_radius_cap` конфига маппится на
    /// capacity). `None` автономии ⇒ confirm (fail-safe). Рефилл — дефолтный ([`DEFAULT_REFILL_TOKENS`]
    /// за [`DEFAULT_REFILL_PER`]). kill-switch НЕвзведён (свежий Arc) — kill-switch проводится через
    /// [`DispatchPolicy::with_paused`]. Для иного рефилла/тест-часов — [`DispatchPolicy::with_bucket`].
    pub fn new(autonomy: Option<&str>, overwrite_threshold: usize, blast_radius_cap: u32) -> Self {
        let bucket = TokenBucket::new(blast_radius_cap, DEFAULT_REFILL_TOKENS, DEFAULT_REFILL_PER);
        Self::with_bucket(autonomy, overwrite_threshold, bucket)
    }

    /// Как [`DispatchPolicy::new`], но с ВНЕШНИМ kill-switch `agent_paused` (process-global Arc проводки
    /// прогона). Токен-бакет — дефолтный из `blast_radius_cap` (как `new`).
    pub fn with_paused(
        autonomy: Option<&str>,
        overwrite_threshold: usize,
        blast_radius_cap: u32,
        agent_paused: Arc<AtomicBool>,
    ) -> Self {
        let bucket = TokenBucket::new(blast_radius_cap, DEFAULT_REFILL_TOKENS, DEFAULT_REFILL_PER);
        Self {
            auto: matches!(autonomy, Some("auto")),
            overwrite_threshold,
            token_bucket: bucket,
            agent_paused,
        }
    }

    /// Как [`DispatchPolicy::new`], но с ГОТОВЫМ токен-бакетом (явный рефилл / инъекция тест-часов).
    /// kill-switch НЕвзведён (см. `new`).
    pub fn with_bucket(
        autonomy: Option<&str>,
        overwrite_threshold: usize,
        token_bucket: TokenBucket,
    ) -> Self {
        Self {
            auto: matches!(autonomy, Some("auto")),
            overwrite_threshold,
            token_bucket,
            agent_paused: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Взведён ли kill-switch (пауза агента) — fail-safe: `true` ⇒ актуатор НЕ должен писать.
    fn is_paused(&self) -> bool {
        self.agent_paused.load(Ordering::Relaxed)
    }
}

/// Результат диспетча одного действия для tool-результата (строка-резюме) + диагностики теста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Применено (авто-Auto под кэпом ИЛИ approved-Confirm/Auto). Несёт резюме (как у [`ApplyOutcome`]).
    Applied(String),
    /// Предложено и ОТКЛОНЕНО решением (Reject) — диск НЕ тронут. Несёт причину-резюме.
    Rejected(String),
    /// Apply упал по безопасной причине (drift/существование/ledger/запись) — диск НЕ изменён.
    Failed(String),
}

impl DispatchOutcome {
    /// Свернуть в `Result<String, ToolError>` для границы инструмента: Applied/Rejected → Ok(резюме);
    /// Failed → ошибка исполнения (зафенсенная — цикл выживает).
    pub fn into_tool_result(self) -> Result<String, ToolError> {
        match self {
            DispatchOutcome::Applied(s) | DispatchOutcome::Rejected(s) => Ok(s),
            DispatchOutcome::Failed(s) => Err(ToolError::Exec(s)),
        }
    }
}

/// Сообщение HardBlocked (зафенсенная ошибка — модель видит причину и переспрашивает).
fn block_message(reason: &BlockReason) -> String {
    match reason {
        BlockReason::PathEscape => {
            "путь вне vault (traversal/абсолютный) — действие заблокировано".to_string()
        }
        BlockReason::ReservedPath => {
            "путь в служебном каталоге (.nexus/.git/dotfile) — действие заблокировано".to_string()
        }
        BlockReason::EmptyPath => "пустой/невалидный путь — действие заблокировано".to_string(),
    }
}

/// Простой line-diff `before → after`: (добавлено, удалено) строк. Не unified-хунки — кумулятивная
/// прикидка (max(after,before)−common) — достаточно для бейджа «+N −M» changeset'а (хунки — AGENT-6).
/// Считаем по строкам с дешёвым LCS-приближением через сопоставление одинаковых строк по позиции — для
/// простоты используем счёт по multiset: общие строки = пересечение мультимножеств, add = after−common,
/// del = before−common. Это стабильно и монотонно для отображения, без зависимостей.
fn line_diff(before: &str, after: &str) -> (u32, u32) {
    use std::collections::HashMap;
    if before == after {
        return (0, 0);
    }
    // Пустой before (create) → все строки after добавлены; пустой after → все before удалены.
    let count_lines = |s: &str| -> u32 {
        if s.is_empty() {
            0
        } else {
            s.lines().count() as u32
        }
    };
    let mut before_ms: HashMap<&str, i64> = HashMap::new();
    for l in before.lines() {
        *before_ms.entry(l).or_insert(0) += 1;
    }
    let mut common: u32 = 0;
    for l in after.lines() {
        if let Some(c) = before_ms.get_mut(l) {
            if *c > 0 {
                *c -= 1;
                common += 1;
            }
        }
    }
    let before_n = count_lines(before);
    let after_n = count_lines(after);
    let add = after_n.saturating_sub(common);
    let del = before_n.saturating_sub(common);
    (add, del)
}

/// Содержимое цели «как будет на диске» после применения действия — для диффа (current → proposed).
/// create/edit: тело действия; frontmatter: результат единственного санкционированного писателя
/// `set_frontmatter_field` поверх current (при ошибке round-trip — current как есть, диф 0/0: апплай
/// сам отвергнет некорректную правку, диф здесь лишь индикатор).
fn proposed_content(action: &Action, current: &str) -> String {
    match &action.target {
        ActionTarget::NoteCreate { .. } | ActionTarget::NoteEdit { .. } => {
            action.content.clone().unwrap_or_default()
        }
        ActionTarget::Frontmatter { key, .. } => {
            let value = action.value.clone().unwrap_or_default();
            crate::parser::set_frontmatter_field(current, key, &value)
                .unwrap_or_else(|_| current.to_string())
        }
    }
}

/// `FileStatus` по виду действия: create → New, edit/frontmatter → Edit.
fn file_status(action: &Action) -> FileStatus {
    match &action.target {
        ActionTarget::NoteCreate { .. } => FileStatus::New,
        ActionTarget::NoteEdit { .. } | ActionTarget::Frontmatter { .. } => FileStatus::Edit,
    }
}

/// БЕЗОПАСНОЕ чтение текущего содержимого цели IN-VAULT (для classify_hash + базы диффа). Резолвит путь
/// через `resolve_vault_path_for_write` (канонизация родителя + конфайнмент) и отвергает leaf-симлинк —
/// зеркало рубежа 1 apply, чтобы НЕ прочитать внешний файл сквозь симлинк (info-leak в диф/хеш). Любой
/// побег/ошибка/несуществование ⇒ `None` (трактуем как «файла нет»); реальную запись всё равно гейтит
/// apply со своим полным рубежом. None ⇒ classify_hash="" (конвенция apply Рубежа 3 для отсутствия).
async fn read_current_in_vault(canon_root: &Path, rel: &str) -> Option<String> {
    let canon_root = canon_root.to_path_buf();
    let rel = rel.to_string();
    tokio::task::spawn_blocking(move || {
        let rel_path = std::path::PathBuf::from(&rel);
        let abs = crate::vault::resolve_vault_path_for_write(&canon_root, &rel_path).ok()?;
        // leaf-симлинк (в т.ч. наружу) ⇒ не читаем (зеркало apply рубежа 1).
        if let Ok(meta) = std::fs::symlink_metadata(&abs) {
            if meta.file_type().is_symlink() {
                return None;
            }
        }
        std::fs::read_to_string(&abs).ok()
    })
    .await
    .ok()
    .flatten()
}

/// Диспетч одного классифицированного действия по матрице `(RiskTier × autonomy)` (см. модульную доку).
///
/// Применяет (Auto-тир в auto-прогоне под blast-radius-кэпом) ЛИБО предлагает (Confirm-тир при ЛЮБОЙ
/// автономии; Auto-тир в confirm-прогоне; Auto-тир за кэпом) — на предложении эмитит Proposal+Diff,
/// пишет ledger-строку `proposed`, спрашивает [`DecisionSource`] и применяет ТОЛЬКО одобренное.
/// HardBlocked ⇒ [`ToolError::Exec`] всегда. `classify_hash` ОБЯЗАТЕЛЬНО передаётся в [`apply_action`].
pub async fn dispatch_action(
    action: &Action,
    run_id: i64,
    policy: &DispatchPolicy,
    decision_source: &Arc<dyn DecisionSource>,
    events: &dyn EventSink,
    ledger: &AuditSink,
    canon_root: &Path,
) -> Result<DispatchOutcome, ToolError> {
    let rel = action.target.rel().to_string();

    // (1) Текущее содержимое цели IN-VAULT → classify_hash (токен на момент classify) + база диффа.
    // None (нет файла / побег) ⇒ classify_hash="" (конвенция apply Рубежа 3: on_disk_hash.unwrap_or("")).
    let current = read_current_in_vault(canon_root, &rel).await;
    let classify_hash: String = current
        .as_deref()
        .map(|c| crate::vault::content_hash(c.as_bytes()))
        .unwrap_or_default();

    // (2) classify с порогом ИЗ КОНФИГА (не 64KiB-константа).
    let ctx = ClassifyCtx {
        root: canon_root,
        overwrite_threshold: policy.overwrite_threshold,
    };
    let tier = classify(action, &ctx);

    // (3) Матч (тир, автономия).
    match &tier {
        // HardBlocked — ВСЕГДА ToolError (апрув не разблокирует; auto не помогает). Диск не трогаем.
        RiskTier::HardBlocked(reason) => Err(ToolError::Exec(block_message(reason))),

        // Auto-тир.
        RiskTier::Auto => {
            // KILL-SWITCH (AGENT-5, чек-пойнт #3): под паузой Auto НЕ авто-применяется. `!is_paused()`
            // ПЕРЕД claim'ом → под паузой токен НЕ тратится и путь уходит в propose (а там apply тоже
            // под-guard'ен, см. propose_and_decide) → НИ ОДНОЙ записи в vault, пока пауза взведена.
            // auto-прогон + НЕ пауза + успешный CLAIM токена ⇒ применить СРАЗУ. claim-before-apply
            // (AGENT-5): токен бронируется АТОМАРНО ДО apply, поэтому конкурентные диспетчи не превысят
            // ёмкость (нет 3d check-then-bump гонки). НЕ-Applied (Failed) ⇒ РЕФАНД (реально потрачен лишь
            // применённый Auto). Короткозамыкаем `&&`: при confirm/паузе claim НЕ зовётся (токен не
            // тратится зря на путь, который всё равно предложит).
            if policy.auto && !policy.is_paused() && policy.token_bucket.try_claim() {
                let out = apply_now(
                    action,
                    run_id,
                    canon_root,
                    ledger,
                    &classify_hash,
                    &policy.agent_paused,
                )
                .await;
                // Токен уже заклеймлен. Applied ⇒ оставляем потраченным; иначе (Failed) ⇒ рефанд.
                if !matches!(out, DispatchOutcome::Applied(_)) {
                    policy.token_bucket.refund();
                }
                Ok(out)
            } else {
                // confirm-прогон (предлагать всё) ИЛИ auto с ПУСТЫМ бакетом (анти-усталость) ИЛИ ПАУЗА
                // (kill-switch) ⇒ предложить (apply под-guard'ен паузой внутри).
                propose_and_decide(
                    action,
                    run_id,
                    &tier,
                    &classify_hash,
                    current.as_deref().unwrap_or(""),
                    decision_source,
                    events,
                    ledger,
                    canon_root,
                    &policy.agent_paused,
                )
                .await
            }
        }

        // Confirm-тир — предложить + ждать решения при ЛЮБОЙ автономии (auto НЕ перекрывает Confirm!).
        RiskTier::Confirm(_) => {
            propose_and_decide(
                action,
                run_id,
                &tier,
                &classify_hash,
                current.as_deref().unwrap_or(""),
                decision_source,
                events,
                ledger,
                canon_root,
                &policy.agent_paused,
            )
            .await
        }
    }
}

/// Применить действие через [`apply_action`] с ОБЯЗАТЕЛЬНЫМ `classify_hash` (3c hard-gate) и свернуть
/// [`ApplyOutcome`] в [`DispatchOutcome`].
///
/// ## KILL-SWITCH LAST-MOMENT RE-CHECK (AGENT-5, сужение TOCTOU)
/// `apply_now` — ЕДИНСТВЕННЫЙ применяющий путь (зовётся из Auto-авто-ветки И из approved-propose-ветки),
/// поэтому здесь стоит ФИНАЛЬНЫЙ страж паузы: `agent_paused` читается В САМОМ НАЧАЛЕ, ДО любого
/// `apply_action`/atomic_write. Вызыватели тоже проверяют паузу (Auto-короткозамыкание; approved-путь
/// re-check после decide()), но между их проверкой и физической записью есть суб-мс окно — флаг мог
/// флипнуться в паузу именно там. Этот guard ЗАКРЫВАЕТ это окно: если пауза взведена → no-op
/// ([`DispatchOutcome::Rejected`]), БЕЗ записи; строка action/proposal остаётся в НЕприменённом
/// состоянии (apply_action не зовётся → ledger executed-строку не пишет). Так инвариант «paused ⇒ нет
/// записи» держится, даже если пауза флипнется между проверкой вызывателя и записью.
async fn apply_now(
    action: &Action,
    run_id: i64,
    canon_root: &Path,
    ledger: &AuditSink,
    classify_hash: &str,
    agent_paused: &Arc<AtomicBool>,
) -> DispatchOutcome {
    // LAST-MOMENT kill-switch: пауза могла взвестись между проверкой вызывателя и этой записью (TOCTOU).
    // Читаем ПЕРЕД apply_action → под паузой НИ ОДНОЙ записи / ledger-executed-строки (no-op Rejected).
    if agent_paused.load(Ordering::Relaxed) {
        return DispatchOutcome::Rejected(format!(
            "применение {} подавлено: агент на паузе (kill-switch взведён в последний момент) — \
             запись НЕ выполнена",
            action.target.rel()
        ));
    }
    match apply_action(action, run_id, canon_root, ledger, Some(classify_hash)).await {
        ApplyOutcome::Executed { summary, .. } => DispatchOutcome::Applied(summary),
        ApplyOutcome::AlreadyDone(outcome) => {
            DispatchOutcome::Applied(format!("уже применено ранее (идемпотентно): {outcome}"))
        }
        ApplyOutcome::PathEscape => DispatchOutcome::Failed(format!(
            "путь {} разрешился ВНЕ vault (симлинк-побег) — запись заблокирована",
            action.target.rel()
        )),
        ApplyOutcome::Failed(reason) => DispatchOutcome::Failed(reason),
    }
}

/// Предложить (ledger `proposed` + эмиссия Proposal/Diff), спросить [`DecisionSource`] и применить
/// ТОЛЬКО при явном Approve (иначе Reject — диск не трогаем). Один айтем на вызов (батч = строки
/// `proposed` прогона; здесь — одно действие за диспетч, что и есть батч из одного айтема).
#[allow(clippy::too_many_arguments)]
async fn propose_and_decide(
    action: &Action,
    run_id: i64,
    tier: &RiskTier,
    classify_hash: &str,
    current: &str,
    decision_source: &Arc<dyn DecisionSource>,
    events: &dyn EventSink,
    ledger: &AuditSink,
    canon_root: &Path,
    agent_paused: &Arc<AtomicBool>,
) -> Result<DispatchOutcome, ToolError> {
    let rel = action.target.rel().to_string();

    // Диф current → proposed.
    let proposed = proposed_content(action, current);
    let (add, del) = line_diff(current, &proposed);
    let status = file_status(action);

    // (4) Ledger-строка state=proposed (НЕтерминальна; решат transition/finish). Ключ ПРЕДЛОЖЕНИЯ
    // ОТДЕЛЁН от ключа apply (префикс "propose:") — иначе record_before самого apply словил бы UNIQUE-
    // дубль и принял approved-строку за CrashedMidExecute. action_id строки proposed адресует решение.
    let propose_key = proposal_key(run_id, action, classify_hash);
    let entry = ActionEntry {
        run_id,
        idempotency_key: propose_key.clone(),
        tool_name: action.target.tool_name().to_string(),
        target_rel: Some(rel.clone()),
        risk_tier: tier.as_str().to_string(),
        state: STATE_PROPOSED.to_string(),
        content_hash: if current.is_empty() {
            None
        } else {
            Some(classify_hash.to_string())
        },
        diff_summary: Some(format!("+{add} -{del}")),
    };
    let action_id = match ledger.record_before(entry).await {
        Ok(id) => id,
        // Дубль ключа предложения (то же действие повторно предложено в прогоне) — берём существующую
        // строку как айтем (идемпотентность предложения). Любая иная ошибка ledger ⇒ Failed (fail-closed).
        Err(_) => match audit::lookup_id(&ledger_reader(ledger), &propose_key).await {
            Some(id) => id,
            None => {
                return Ok(DispatchOutcome::Failed(
                    "ledger: не удалось записать строку предложения".to_string(),
                ))
            }
        },
    };

    // Эмиссия Proposal (батч из одного айтема) + пер-файловый Diff (CONTRACT-NOTES поверхность аппрува).
    let file = ProposedFile {
        path: rel.clone(),
        add,
        del,
        status,
        action_id,
    };
    events.emit(AgentEvent::Proposal {
        run_id,
        files: vec![file],
    });
    events.emit(AgentEvent::Diff {
        path: rel.clone(),
        add,
        del,
        status,
    });

    // Спросить источник решений.
    let batch = ProposalBatch {
        run_id,
        items: vec![ProposalItem {
            action_id,
            target_rel: rel.clone(),
            tier: tier.clone(),
            add,
            del,
        }],
    };
    let decision = decision_source.decide(&batch).await;

    match decision.decision_for(action_id) {
        // Approve ⇒ proposed→approved (state, без outcome) ⇒ apply (с classify_hash). Если transition
        // не применился (гонка/двойное решение/чужое состояние) — fail-closed: НЕ применяем.
        ItemDecision::Approve => {
            // KILL-SWITCH (AGENT-5, чек-пойнт #3): даже ОДОБРЕННОЕ предложение НЕ пишется под паузой.
            // Re-check ПОСЛЕ decide() (источник мог думать долго / пауза взведена в это окно) и ПЕРЕД
            // любым transition/apply. Строку оставляем `proposed` (НЕ approved) → её можно одобрить
            // снова на un-pause. Это финальный страж: одобряющий DecisionSource не пробьёт паузу в запись.
            if agent_paused.load(Ordering::Relaxed) {
                return Ok(DispatchOutcome::Rejected(format!(
                    "предложение {rel}: агент на паузе (kill-switch) — запись подавлена (предложение \
                     остаётся для повторного решения на un-pause)"
                )));
            }
            let promoted = audit::transition(
                &ledger_writer(ledger),
                &propose_key,
                STATE_PROPOSED,
                STATE_APPROVED,
            )
            .await
            .unwrap_or(false);
            if !promoted {
                return Ok(DispatchOutcome::Failed(format!(
                    "предложение {rel}: одобрение не применено (строка не в состоянии proposed) — \
                     запись отменена"
                )));
            }
            Ok(apply_now(
                action,
                run_id,
                canon_root,
                ledger,
                classify_hash,
                agent_paused,
            )
            .await)
        }
        // Reject ⇒ proposed→rejected (finish с исходом, терминал). Диск НЕ тронут.
        ItemDecision::Reject => {
            let outcome = format!("предложение {rel} отклонено — НЕ применено");
            let _ = ledger
                .finish(&propose_key, STATE_REJECTED, &outcome, None)
                .await;
            Ok(DispatchOutcome::Rejected(outcome))
        }
    }
}

/// Ключ строки ПРЕДЛОЖЕНИЯ — отдельный от apply-ключа (префикс), чтобы не коллизировать с record_before
/// самого apply. Стабилен по `(run_id, tool, args, classify_hash)` — то же предложение даёт тот же ключ.
fn proposal_key(run_id: i64, action: &Action, classify_hash: &str) -> String {
    let payload = match &action.target {
        ActionTarget::NoteCreate { .. } | ActionTarget::NoteEdit { .. } => {
            action.content.as_deref()
        }
        ActionTarget::Frontmatter { .. } => action.value.as_deref(),
    };
    let args = canonical_args(Some(action.target.rel()), payload);
    let base = idempotency_key(run_id, action.target.tool_name(), &args, classify_hash);
    format!("propose:{base}")
}

// AuditSink держит writer/reader приватными; гейту нужны оба для transition/lookup. Минимальные
// аксессоры через публичный API sink'а (clone дёшев, ADR-003) — без расширения публичной поверхности
// внутренними полями. Реализованы через методы AuditSink ниже (см. apply.rs).
fn ledger_writer(sink: &AuditSink) -> crate::db::WriteActor {
    sink.writer_handle()
}
fn ledger_reader(sink: &AuditSink) -> crate::db::ReadPool {
    sink.reader_handle()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::audit::{lookup, STATE_EXECUTED, STATE_PROPOSED};
    use crate::actuator::decision::{BatchDecision, ChannelDecision, PolicyDefault};
    use crate::db::Database;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Временный vault + БД + sink. canon_root КАНОНИЗИРОВАН (предусловие resolve_vault_path_for_write).
    async fn setup() -> (TempDir, PathBuf, AuditSink) {
        let dir = TempDir::new().unwrap();
        let canon_root = dir.path().canonicalize().unwrap();
        let db = Database::open(canon_root.join(".nexus/nexus.db"))
            .await
            .unwrap();
        let sink = AuditSink::new(db.writer().clone(), db.reader().clone());
        std::mem::forget(db); // writer/reader клонированы в sink — актор жив, пока жив клон.
        (dir, canon_root, sink)
    }

    fn write_existing(root: &Path, rel: &str, content: &str) {
        let abs = root.join(rel);
        if let Some(p) = abs.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(abs, content).unwrap();
    }

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    /// Стандартный порог теста (мал, чтобы крупная правка легко перешагнула).
    const T: usize = 100;
    /// Ёмкость токен-бакета теста.
    const CAP: u32 = 3;

    fn policy(autonomy: Option<&str>) -> DispatchPolicy {
        DispatchPolicy::new(autonomy, T, CAP)
    }

    fn approve(action_id: i64) -> BatchDecision {
        BatchDecision::from_pairs([(action_id, ItemDecision::Approve)])
    }

    /// Снять единственный action_id из эмитированного Proposal (для адресации решения в тесте).
    fn proposed_action_id(sink: &CollectingSink) -> i64 {
        for ev in sink.events() {
            if let AgentEvent::Proposal { files, .. } = ev {
                return files[0].action_id;
            }
        }
        panic!("Proposal не эмитирован");
    }

    // Примечание про адресацию решения в тестах: строка `proposed` — первый INSERT в пустую БД ⇒
    // action_id=1, поэтому Approve-решения засеиваются по id=1 (для надёжности тесты также читают id
    // из эмитированного Proposal). Источники: PolicyDefault (reject-all) или ChannelDecision (засев).

    /// confirm-run + Auto-тир ⇒ ПРЕДЛАГАЕТ (Proposal+Diff, ledger proposed, файл НЕ записан до Approve).
    #[tokio::test]
    async fn confirm_run_auto_tier_proposes_not_applied() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        // Источник, который ОТКЛОНЯЕТ (чтобы проверить «не записано до Approve»).
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
        let action = Action::note_create("Notes/N.md", "hi");

        let out = dispatch_action(
            &action,
            1,
            &policy(Some("confirm")),
            &src,
            &events,
            &sink,
            &root,
        )
        .await
        .unwrap();

        // Предложено и отклонено (PolicyDefault) — файл НЕ создан.
        assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
        assert!(!root.join("Notes/N.md").exists(), "файл НЕ записан");

        // Эмитированы Proposal + Diff с корректной формой.
        let evs = events.events();
        let proposal = evs
            .iter()
            .find(|e| matches!(e, AgentEvent::Proposal { .. }))
            .expect("Proposal эмитирован");
        if let AgentEvent::Proposal { run_id, files } = proposal {
            assert_eq!(*run_id, 1);
            assert_eq!(files[0].path, "Notes/N.md");
            assert_eq!(files[0].status, FileStatus::New);
            assert_eq!(files[0].add, 1, "одна строка добавлена (create)");
        }
        assert!(
            evs.iter().any(|e| matches!(e, AgentEvent::Diff { .. })),
            "Diff эмитирован"
        );

        // Ledger: строка proposed → rejected (терминал с исходом).
        let key = proposal_key(1, &action, "");
        let row = lookup(&sink_reader(&sink), &key).await.unwrap().unwrap();
        assert_eq!(row.state, STATE_REJECTED);
        assert!(row.outcome.is_some());
    }

    /// confirm-run + Auto-тир + Approve ⇒ ПРИМЕНЯЕТ (файл записан, ledger executed для apply-строки).
    #[tokio::test]
    async fn confirm_run_approve_applies() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        // action_id строки proposed в пустой БД = 1 (первый INSERT). Засеваем Approve по id=1.
        let (chan, tx) = ChannelDecision::new(1);
        tx.send(approve(1)).await.unwrap();
        let src: Arc<dyn DecisionSource> = Arc::new(chan);
        let action = Action::note_create("Notes/N.md", "hello");

        let out = dispatch_action(
            &action,
            1,
            &policy(Some("confirm")),
            &src,
            &events,
            &sink,
            &root,
        )
        .await
        .unwrap();

        assert!(matches!(out, DispatchOutcome::Applied(_)), "out={out:?}");
        assert_eq!(
            read(&root, "Notes/N.md"),
            "hello",
            "файл записан после Approve"
        );
        assert_eq!(proposed_action_id(&events), 1, "action_id предложения = 1");

        // Ledger: proposed-строка одобрена (approved), а apply записал СВОЮ executed-строку.
        let pkey = proposal_key(1, &action, "");
        let prow = lookup(&sink_reader(&sink), &pkey).await.unwrap().unwrap();
        assert_eq!(prow.state, STATE_APPROVED, "proposed→approved");
        assert!(
            prow.outcome.is_none(),
            "approved НЕ терминальна (apply отдельно)"
        );
    }

    /// confirm-run + Reject ⇒ ledger rejected, файл НЕ записан.
    #[tokio::test]
    async fn confirm_run_reject_no_write() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let (chan, tx) = ChannelDecision::new(1);
        tx.send(BatchDecision::from_pairs([(1, ItemDecision::Reject)]))
            .await
            .unwrap();
        let src: Arc<dyn DecisionSource> = Arc::new(chan);
        let action = Action::note_create("R.md", "x");

        let out = dispatch_action(
            &action,
            1,
            &policy(Some("confirm")),
            &src,
            &events,
            &sink,
            &root,
        )
        .await
        .unwrap();
        assert!(matches!(out, DispatchOutcome::Rejected(_)));
        assert!(!root.join("R.md").exists());
        let key = proposal_key(1, &action, "");
        let row = lookup(&sink_reader(&sink), &key).await.unwrap().unwrap();
        assert_eq!(row.state, STATE_REJECTED);
    }

    /// auto-run + Auto-тир ⇒ ПРИМЕНЯЕТ напрямую (НЕ предложение), токен бакета потрачен.
    #[tokio::test]
    async fn auto_run_auto_tier_applies_directly_spends_token() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // не должен быть спрошен.
        let pol = policy(Some("auto"));
        let action = Action::note_create("A.md", "auto-body");

        let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        assert!(matches!(out, DispatchOutcome::Applied(_)), "out={out:?}");
        assert_eq!(read(&root, "A.md"), "auto-body");
        assert_eq!(
            pol.token_bucket.available(),
            CAP - 1,
            "один токен заклеймлен (CAP={CAP})"
        );
        // НИ Proposal, НИ Diff (применено напрямую).
        assert!(
            !events
                .events()
                .iter()
                .any(|e| matches!(e, AgentEvent::Proposal { .. } | AgentEvent::Diff { .. })),
            "авто-применение НЕ эмитит предложение"
        );
    }

    /// auto-run + Auto-тир с ПУСТЫМ бакетом (capacity=0) ⇒ ФОРСИРУЕТ предложение (анти-усталость).
    #[tokio::test]
    async fn auto_run_empty_bucket_forces_proposal() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // reject all.
                                                                    // Ёмкость = 0 ⇒ даже первое Auto-действие не может заклеймить токен ⇒ предложение.
        let pol = DispatchPolicy::new(Some("auto"), T, 0);
        let action = Action::note_create("Cap.md", "x");

        let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        // PolicyDefault reject ⇒ Rejected, файл НЕ записан (форс-предложение реально предложило).
        assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
        assert!(!root.join("Cap.md").exists());
        assert!(
            events
                .events()
                .iter()
                .any(|e| matches!(e, AgentEvent::Proposal { .. })),
            "пустой бакет — предложение"
        );
    }

    /// Токен-бакет ТОЧНАЯ граница ёмкости (общий бакет прогона): capacity=2 ⇒ ПЕРВЫЕ ДВА Auto
    /// авто-применяются, ТРЕТЬЕ форсирует предложение (кумулятивно по диспетчам одной политики). На
    /// [`ManualClock`] (без продвижения) рефилл НЕ срабатывает — проверяем чистую ёмкость.
    #[tokio::test]
    async fn token_bucket_boundary_capacity_then_propose() {
        let (_d, root, sink) = setup().await;
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // reject (для 3-го предложения).
                                                                    // capacity=2, рефилл 1/окно, но часы НЕ двигаем → рефилла нет (чистая ёмкость).
        let clock = Arc::new(ManualClock::new());
        let bucket = TokenBucket::with_clock(2, 1, Duration::from_secs(60), clock);
        let pol = DispatchPolicy::with_bucket(Some("auto"), T, bucket);

        // Действие 1 и 2 — Auto, под ёмкостью ⇒ применяются.
        for (i, rel) in ["B1.md", "B2.md"].iter().enumerate() {
            let events = CollectingSink::new();
            let action = Action::note_create(*rel, "x");
            let out = dispatch_action(&action, (i + 1) as i64, &pol, &src, &events, &sink, &root)
                .await
                .unwrap();
            assert!(matches!(out, DispatchOutcome::Applied(_)), "{rel}: {out:?}");
            assert!(root.join(rel).exists(), "{rel} записан");
        }
        assert_eq!(
            pol.token_bucket.available(),
            0,
            "два токена потрачены — бакет пуст"
        );

        // Действие 3 — Auto, но бакет ПУСТ ⇒ предложение (PolicyDefault reject ⇒ не записано).
        let events = CollectingSink::new();
        let action = Action::note_create("B3.md", "x");
        let out = dispatch_action(&action, 3, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        assert!(matches!(out, DispatchOutcome::Rejected(_)), "3-е: {out:?}");
        assert!(
            !root.join("B3.md").exists(),
            "3-е НЕ записано (бакет пуст → предложено)"
        );
        assert!(
            events
                .events()
                .iter()
                .any(|e| matches!(e, AgentEvent::Proposal { .. })),
            "3-е действие предложено"
        );
        assert_eq!(
            pol.token_bucket.available(),
            0,
            "предложение не тратит токен (бакет остался пуст)"
        );
    }

    /// auto-run + Confirm-тир (крупная перезапись) ⇒ ВСЁ РАВНО предлагает (auto НЕ перекрывает Confirm).
    #[tokio::test]
    async fn auto_run_confirm_tier_still_proposes() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "E.md", "orig");
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // reject all.
        let pol = policy(Some("auto")); // auto, но Confirm-тир НЕ должен авто-примениться.
        let big = "y".repeat(T + 1);
        let action = Action::note_edit("E.md", big);

        let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        // Предложено (Confirm) и отклонено PolicyDefault ⇒ файл НЕ перезаписан, токен НЕ потрачен.
        assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
        assert_eq!(read(&root, "E.md"), "orig", "Confirm в auto НЕ применился");
        assert_eq!(
            pol.token_bucket.available(),
            CAP,
            "Confirm не клеймит токен (бакет полон)"
        );
        assert!(
            events
                .events()
                .iter()
                .any(|e| matches!(e, AgentEvent::Proposal { .. })),
            "Confirm-тир в auto — предложение"
        );
    }

    /// PolicyDefault: confirm-run под ним НИКОГДА не применяет Confirm-тир (fail-closed).
    #[tokio::test]
    async fn policy_default_never_applies_confirm() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "E.md", "orig");
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
        let big = "z".repeat(T + 1);
        let action = Action::note_edit("E.md", big);

        let out = dispatch_action(
            &action,
            1,
            &policy(Some("confirm")),
            &src,
            &events,
            &sink,
            &root,
        )
        .await
        .unwrap();
        assert!(matches!(out, DispatchOutcome::Rejected(_)));
        assert_eq!(
            read(&root, "E.md"),
            "orig",
            "PolicyDefault не применил Confirm"
        );
    }

    /// classify_hash threaded: дрейф МЕЖДУ propose и approve ⇒ apply отменяет Failed(drift), без клоббера.
    #[tokio::test]
    async fn drift_between_propose_and_approve_aborts() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "E.md", "orig-content");
        let events = CollectingSink::new();
        // Источник, который ПЕРЕД ответом Approve портит файл на диске (внешний писатель) — но решение
        // шлём через канал ПОСЛЕ ручной мутации. Здесь: засеваем Approve по id=1, а дрейф вносим
        // мутацией файла ДО диспетча? Нет — нужен дрейф ПОСЛЕ classify, ДО apply. Делаем кастомный
        // источник, который мутирует файл внутри decide(), затем одобряет.
        struct DriftThenApprove {
            root: PathBuf,
        }
        #[async_trait::async_trait]
        impl DecisionSource for DriftThenApprove {
            async fn decide(&self, batch: &ProposalBatch) -> BatchDecision {
                // Внешний писатель меняет файл МЕЖДУ classify (в dispatch) и apply (после approve).
                fs::write(self.root.join("E.md"), "EXTERNALLY-CHANGED").unwrap();
                BatchDecision::from_pairs([(batch.items[0].action_id, ItemDecision::Approve)])
            }
        }
        let src: Arc<dyn DecisionSource> = Arc::new(DriftThenApprove { root: root.clone() });
        // Малая правка ⇒ Auto-тир, но confirm-run ⇒ предложение (чтобы пройти propose→approve→apply).
        let action = Action::note_edit("E.md", "small new body");

        let out = dispatch_action(
            &action,
            1,
            &policy(Some("confirm")),
            &src,
            &events,
            &sink,
            &root,
        )
        .await
        .unwrap();
        // apply Рубеж 3: on-disk hash (EXTERNALLY-CHANGED) != classify_hash (orig-content) ⇒ Failed(drift).
        assert!(matches!(out, DispatchOutcome::Failed(_)), "out={out:?}");
        assert_eq!(
            read(&root, "E.md"),
            "EXTERNALLY-CHANGED",
            "наша правка НЕ затёрла внешнюю (анти-клоббер)"
        );
    }

    /// overwrite_threshold ИЗ КОНФИГА уважается: правка > threshold ⇒ Confirm (предложение), даже в auto.
    #[tokio::test]
    async fn config_overwrite_threshold_respected() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "E.md", "orig");
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
        // Порог из конфига = 10 байт; правка 11 байт ⇒ Confirm.
        let pol = DispatchPolicy::new(Some("auto"), 10, CAP);
        let action = Action::note_edit("E.md", "12345678901"); // 11 байт > 10.

        let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        assert!(
            matches!(out, DispatchOutcome::Rejected(_)),
            "Confirm из конфиг-порога"
        );
        assert!(events
            .events()
            .iter()
            .any(|e| matches!(e, AgentEvent::Proposal { .. })));

        // Та же правка под БОЛЬШИМ порогом (1000) ⇒ Auto ⇒ авто-применяется в auto-прогоне.
        let events2 = CollectingSink::new();
        let src2: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
        let pol2 = DispatchPolicy::new(Some("auto"), 1000, CAP);
        let out2 = dispatch_action(&action, 2, &pol2, &src2, &events2, &sink, &root)
            .await
            .unwrap();
        assert!(
            matches!(out2, DispatchOutcome::Applied(_)),
            "под порогом — Auto-apply"
        );
        assert_eq!(read(&root, "E.md"), "12345678901");
    }

    /// HardBlocked (escape) ⇒ ToolError при ЛЮБОЙ автономии; диск не тронут; нет предложения.
    #[tokio::test]
    async fn hardblocked_errors_any_autonomy() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
        let action = Action::note_create("../escape.md", "x");

        for autonomy in [Some("auto"), Some("confirm"), None] {
            let r =
                dispatch_action(&action, 1, &policy(autonomy), &src, &events, &sink, &root).await;
            assert!(
                matches!(r, Err(ToolError::Exec(_))),
                "autonomy={autonomy:?}"
            );
        }
        assert!(!root.join("../escape.md").exists());
        assert!(
            events.events().is_empty(),
            "HardBlocked не эмитит предложение"
        );
    }

    /// None автономии трактуется как confirm (безопаснее): Auto-тир предлагается, не авто-применяется.
    #[tokio::test]
    async fn none_autonomy_defaults_to_confirm() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
        let action = Action::note_create("N.md", "x");

        let out = dispatch_action(&action, 1, &policy(None), &src, &events, &sink, &root)
            .await
            .unwrap();
        assert!(
            matches!(out, DispatchOutcome::Rejected(_)),
            "None ⇒ confirm ⇒ предложение"
        );
        assert!(!root.join("N.md").exists());
    }

    /// Диф line-count: create (пусто → N строк) и edit (правка строк).
    #[test]
    fn line_diff_counts() {
        assert_eq!(line_diff("", "a\nb\nc"), (3, 0), "create — 3 add");
        assert_eq!(line_diff("a\nb\nc", ""), (0, 3), "очистка — 3 del");
        assert_eq!(line_diff("a\nb\nc", "a\nX\nc"), (1, 1), "1 строка изменена");
        assert_eq!(line_diff("same", "same"), (0, 0), "идентично — 0/0");
    }

    /// apply-строка после Approve реально executed (полный путь propose→approve→apply→ledger).
    #[tokio::test]
    async fn approved_apply_row_is_executed() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let (chan, tx) = ChannelDecision::new(1);
        tx.send(approve(1)).await.unwrap();
        let src: Arc<dyn DecisionSource> = Arc::new(chan);
        let action = Action::note_create("Ok.md", "done");

        dispatch_action(
            &action,
            1,
            &policy(Some("confirm")),
            &src,
            &events,
            &sink,
            &root,
        )
        .await
        .unwrap();
        assert_eq!(read(&root, "Ok.md"), "done");
        // apply-ключ (без propose-префикса): для create — target_hash = хеш планируемого тела (apply
        // fallback при None? нет — здесь classify_hash="" передан). Найдём executed-строку по run_id.
        let n_executed: i64 = sink_reader(&sink)
            .query(|c| {
                c.query_row(
                    "SELECT count(*) FROM agent_actions WHERE run_id=1 AND state=?1",
                    [STATE_EXECUTED],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(n_executed, 1, "ровно одна executed apply-строка");
    }

    // ── AGENT-5: KILL-SWITCH (чек-пойнт #3 — актуатор НЕ пишет под паузой) ─────────────────────────

    /// Политика с ВЗВЕДЁННЫМ kill-switch (пауза) — auto-прогон, но писать нельзя.
    fn paused_policy(autonomy: Option<&str>) -> DispatchPolicy {
        DispatchPolicy::with_paused(autonomy, T, CAP, Arc::new(AtomicBool::new(true)))
    }

    /// **KILL-SWITCH чек-пойнт #3 (auto-тир, auto-прогон, ПАУЗА)**: даже Auto-тир в auto-прогоне НЕ
    /// авто-применяется под паузой — форс-предложение, и под PolicyDefault (auto-DENY) файл НЕ записан.
    /// Токен НЕ потрачен (claim не зовётся под паузой). Доказывает: пауза блокирует авто-запись.
    #[tokio::test]
    async fn paused_auto_tier_does_not_apply() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault);
        let pol = paused_policy(Some("auto"));
        let action = Action::note_create("Paused.md", "x");

        let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        // Под паузой Auto уходит в propose; PolicyDefault reject ⇒ Rejected, файл НЕ записан.
        assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
        assert!(
            !root.join("Paused.md").exists(),
            "под паузой файл НЕ записан"
        );
        assert_eq!(
            pol.token_bucket.available(),
            CAP,
            "под паузой claim не зовётся — токен НЕ потрачен"
        );
    }

    /// **KILL-SWITCH чек-пойнт #3 (ОДОБРЕНО, но ПАУЗА → НЕ записано)** — самый жёсткий тест: даже
    /// DecisionSource, который ОДОБРЯЕТ (Approve), НЕ пробивает паузу в запись. Re-check паузы ПОСЛЕ
    /// decide() ПЕРЕД apply ⇒ Rejected, файл НЕ записан, строка остаётся `proposed` (можно одобрить на
    /// un-pause). Это гарантия «paused ⇒ нет записи» даже при approving-источнике.
    #[tokio::test]
    async fn paused_approved_proposal_still_not_written() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        // Источник, который ОДОБРЯЕТ id=1 (строка proposed в пустой БД = 1).
        let (chan, tx) = ChannelDecision::new(1);
        tx.send(approve(1)).await.unwrap();
        let src: Arc<dyn DecisionSource> = Arc::new(chan);
        // confirm-прогон + ПАУЗА: идёт по propose-пути, источник одобряет — но пауза блокирует apply.
        let pol = paused_policy(Some("confirm"));
        let action = Action::note_create("ApprovedButPaused.md", "hi");

        let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        assert!(
            matches!(out, DispatchOutcome::Rejected(_)),
            "одобрено, но пауза ⇒ запись подавлена: {out:?}"
        );
        assert!(
            !root.join("ApprovedButPaused.md").exists(),
            "ОДОБРЕНО, но ПАУЗА → файл НЕ записан (kill-switch пробивает даже Approve)"
        );
        // Строка осталась `proposed` (НЕ approved/executed) — её можно одобрить снова на un-pause.
        let key = proposal_key(1, &action, "");
        let row = lookup(&sink_reader(&sink), &key).await.unwrap().unwrap();
        assert_eq!(
            row.state, STATE_PROPOSED,
            "под паузой строка не повышена до approved (остаётся proposed)"
        );
    }

    /// **KILL-SWITCH LAST-MOMENT GUARD (apply_now, TOCTOU-сужение)** — пауза флипается в `true` ПОСЛЕ
    /// решения Approve, но ДО фактической записи в `apply_now`. Инъекция флипа: кастомный DecisionSource
    /// одобряет и ставит в строй «отложенный флип», который проворачиваем перетиранием флага ПЕРЕД тем,
    /// как `apply_now` доберётся до `apply_action`. Поскольку и approved-путь (:779), и `apply_now` читают
    /// ОДИН Arc, мы доказываем сам guard `apply_now`, вызывая его НАПРЯМУЮ со взведённым флагом: запись
    /// НЕ происходит (файл не создан), `apply_action` не зовётся (ledger executed-строки нет) → no-op
    /// Rejected. Это и есть финальный страж окна между проверкой вызывателя и записью.
    #[tokio::test]
    async fn apply_now_late_pause_blocks_write() {
        let (_d, root, sink) = setup().await;
        // Флаг стартует НЕ на паузе (как если бы проверка вызывателя на :617/:779 уже прошла), затем
        // флипается в паузу В ОКНЕ перед записью — эмулируем это, взводя флаг ДО прямого вызова apply_now.
        let agent_paused = Arc::new(AtomicBool::new(false));
        // Симулируем «вызыватель проверил — было НЕ на паузе»: читаем флаг (false), затем флипаем.
        assert!(
            !agent_paused.load(Ordering::Relaxed),
            "до флипа: НЕ на паузе (как при проверке вызывателя)"
        );
        agent_paused.store(true, Ordering::Relaxed); // пауза взведена В ОКНЕ перед записью.

        let action = Action::note_create("LateP.md", "should-not-be-written");
        // apply_now — единственный применяющий путь; зовём напрямую (как из approved-ветки), classify_hash
        // = "" (create-конвенция). LAST-MOMENT guard читает флаг → Rejected, БЕЗ записи.
        let out = apply_now(&action, 1, &root, &sink, "", &agent_paused).await;

        assert!(
            matches!(out, DispatchOutcome::Rejected(_)),
            "пауза в последний момент ⇒ no-op Rejected: {out:?}"
        );
        assert!(
            !root.join("LateP.md").exists(),
            "LAST-MOMENT guard: файл НЕ записан (пауза взведена после проверки вызывателя, до записи)"
        );
        // apply_action не зван → НИ ОДНОЙ executed-строки ledger для этого прогона.
        let n_executed: i64 = sink_reader(&sink)
            .query(|c| {
                c.query_row(
                    "SELECT count(*) FROM agent_actions WHERE run_id=1 AND state=?1",
                    [STATE_EXECUTED],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(
            n_executed, 0,
            "LAST-MOMENT guard: apply_action не зван — ledger executed-строки нет"
        );
    }

    /// Контр-проверка: тот же путь apply_now БЕЗ паузы реально ПИШЕТ (guard не ложно-срабатывает) —
    /// доказывает, что блокировка выше обусловлена именно паузой, а не сломанным apply_now.
    #[tokio::test]
    async fn apply_now_not_paused_writes() {
        let (_d, root, sink) = setup().await;
        let agent_paused = Arc::new(AtomicBool::new(false)); // НЕ на паузе.
        let action = Action::note_create("LiveP.md", "written");
        let out = apply_now(&action, 1, &root, &sink, "", &agent_paused).await;
        assert!(matches!(out, DispatchOutcome::Applied(_)), "out={out:?}");
        assert_eq!(
            read(&root, "LiveP.md"),
            "written",
            "без паузы apply_now пишет"
        );
    }

    /// **End-to-end флип ПОСЛЕ решения через dispatch_action**: DecisionSource одобряет и ВНУТРИ decide()
    /// взводит ОБЩИЙ с политикой `agent_paused`. Это эмулирует паузу, взведённую в окне принятия решения /
    /// перед записью. Оба стража (re-check :779 И last-moment apply_now) читают этот Arc ⇒ запись
    /// подавлена: файл НЕ создан, строка остаётся proposed. Дополняет прямой unit-тест guard'а сверху
    /// полным путём dispatch→propose→approve→(пауза)→НЕ-запись.
    #[tokio::test]
    async fn dispatch_pause_flip_during_decide_blocks_write() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let agent_paused = Arc::new(AtomicBool::new(false));

        // Источник: одобряет id=1 И взводит общий флаг паузы ВНУТРИ decide() (флип после предложения).
        struct ApproveThenPause {
            flag: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl DecisionSource for ApproveThenPause {
            async fn decide(&self, batch: &ProposalBatch) -> BatchDecision {
                self.flag.store(true, Ordering::Relaxed); // пауза взведена в окне решения.
                BatchDecision::from_pairs([(batch.items[0].action_id, ItemDecision::Approve)])
            }
        }
        let src: Arc<dyn DecisionSource> = Arc::new(ApproveThenPause {
            flag: agent_paused.clone(),
        });
        // confirm-прогон с ОБЩИМ флагом паузы (стартует НЕ на паузе): идёт propose→decide→(флип)→страж.
        let pol = DispatchPolicy::with_paused(Some("confirm"), T, CAP, agent_paused.clone());
        let action = Action::note_create("FlipDuringDecide.md", "x");

        let out = dispatch_action(&action, 1, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        assert!(
            matches!(out, DispatchOutcome::Rejected(_)),
            "пауза взведена в окне решения ⇒ запись подавлена: {out:?}"
        );
        assert!(
            !root.join("FlipDuringDecide.md").exists(),
            "флип после решения ⇒ файл НЕ записан"
        );
        // Строка остаётся proposed (re-check :779 перехватил до transition) — можно одобрить на un-pause.
        let key = proposal_key(1, &action, "");
        let row = lookup(&sink_reader(&sink), &key).await.unwrap().unwrap();
        assert_eq!(
            row.state, STATE_PROPOSED,
            "флип в окне решения: строка осталась proposed"
        );
    }

    // ── AGENT-5: токен-бакет (анти-усталость, claim-before-apply, рефилл, конкурентность) ──────────

    /// **НЕ-Applied НЕ тратит токен (рефанд через dispatch).** auto-прогон, Auto-тир, но apply ПАДАЕТ
    /// (drift: classify_hash≠on-disk) ⇒ Failed. Токен заклеймлен ДО apply, но Failed ⇒ РЕФАНД: бакет
    /// остаётся ПОЛНЫМ. Доказывает, что «потрачен только реально применённый Auto».
    #[tokio::test]
    async fn non_applied_outcome_refunds_token() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "E.md", "orig-content");
        let events = CollectingSink::new();
        // Источник не спрашивается (Auto-тир в auto-прогоне). Дрейф вносим внешним писателем ВНУТРИ
        // несуществующего decide? Нет — Auto-тир НЕ предлагает. Дрейф провоцируем иначе: классифай
        // прочитает "orig-content", а apply Рубеж 3 сверит хэш — совпадёт. Чтобы получить Failed без
        // решения, делаем edit НЕсуществующего файла → apply Failed(не существует). Это НЕ-Applied.
        let pol = policy(Some("auto")); // capacity=CAP, полный.
        assert_eq!(pol.token_bucket.available(), CAP, "бакет стартует полным");
        // note.edit по ОТСУТСТВУЮЩЕМУ файлу: Auto-тир (малый размер) → claim → apply Failed (нет файла).
        let action = Action::note_edit("Missing.md", "small");
        let out = dispatch_action(&action, 1, &pol, &src_reject(), &events, &sink, &root)
            .await
            .unwrap();
        assert!(
            matches!(out, DispatchOutcome::Failed(_)),
            "edit отсутствующего → Failed: {out:?}"
        );
        assert_eq!(
            pol.token_bucket.available(),
            CAP,
            "НЕ-Applied (Failed) → токен возвращён (рефанд): бакет полон"
        );
    }

    /// PolicyDefault как `Arc<dyn DecisionSource>` (reject-all) — хелпер для тестов выше.
    fn src_reject() -> Arc<dyn DecisionSource> {
        Arc::new(PolicyDefault)
    }

    /// Чистая единица: N claim'ов на ёмкости N успешны, (N+1)-й — нет (бакет пуст). Без apply/БД.
    #[test]
    fn token_bucket_capacity_n_then_empty() {
        let clock = Arc::new(ManualClock::new());
        let b = TokenBucket::with_clock(3, 1, Duration::from_secs(60), clock);
        assert_eq!(b.available(), 3, "стартует полным");
        assert!(b.try_claim(), "claim 1");
        assert!(b.try_claim(), "claim 2");
        assert!(b.try_claim(), "claim 3");
        assert!(!b.try_claim(), "claim 4 — бакет пуст");
        assert_eq!(b.available(), 0);
    }

    /// Чистая единица: после РЕФИЛЛ-окна (продвижение ManualClock) ёмкость восстанавливается — но НЕ
    /// выше capacity. Рефилл по времени детерминирован (ручные часы, без sleep/Instant::now()).
    #[test]
    fn token_bucket_refills_after_window() {
        let clock = Arc::new(ManualClock::new());
        // capacity=2, рефилл 1 токен за 10 с.
        let b = TokenBucket::with_clock(2, 1, Duration::from_secs(10), clock.clone());
        assert!(b.try_claim() && b.try_claim(), "опустошаем бакет");
        assert!(!b.try_claim(), "пуст");

        // Прошло одно окно (10 с) → доначислен 1 токен.
        clock.advance(Duration::from_secs(10));
        assert_eq!(b.available(), 1, "одно окно → +1 токен");
        assert!(b.try_claim(), "claim восстановленного токена");
        assert!(!b.try_claim(), "снова пуст");

        // Прошло ТРИ окна сразу (30 с) → доначислено 3, но потолок capacity=2.
        clock.advance(Duration::from_secs(30));
        assert_eq!(
            b.available(),
            2,
            "много окон → не выше capacity (потолок 2)"
        );
    }

    /// Чистая единица: ДРОБНОЕ окно (меньше refill_per) НЕ доначисляет токен, а остаток времени НЕ
    /// теряется — накопившись до полного окна, токен доначисляется. (`last_refill` продвигается на
    /// целые окна, не на текущий момент.)
    #[test]
    fn token_bucket_partial_window_does_not_credit_but_accumulates() {
        let clock = Arc::new(ManualClock::new());
        let b = TokenBucket::with_clock(1, 1, Duration::from_secs(10), clock.clone());
        assert!(b.try_claim(), "опустошаем (capacity=1)");
        assert!(!b.try_claim(), "пуст");

        // 6 с < 10 с → НЕ доначисляет.
        clock.advance(Duration::from_secs(6));
        assert_eq!(b.available(), 0, "дробное окно (6с) не доначисляет");
        // ещё 6 с → суммарно 12 с ≥ одно окно (10 с) → +1 токен (остаток 2 с не потерян).
        clock.advance(Duration::from_secs(6));
        assert_eq!(b.available(), 1, "накоплено полное окно → +1 токен");
    }

    /// Чистая единица: refund НЕ превышает capacity (потолок). Рефанд без предшествующего claim не
    /// «раздувает» бакет сверх ёмкости.
    #[test]
    fn token_bucket_refund_capped_at_capacity() {
        let clock = Arc::new(ManualClock::new());
        let b = TokenBucket::with_clock(2, 0, Duration::ZERO, clock); // без рефилла по времени.
        assert_eq!(b.available(), 2, "полон");
        b.refund(); // уже полон → потолок не превышен.
        assert_eq!(
            b.available(),
            2,
            "refund на полном бакете не превышает capacity"
        );
        assert!(b.try_claim());
        b.refund();
        assert_eq!(b.available(), 2, "claim+refund ⇒ обратно полон, не выше");
    }

    /// **CONCURRENCY-SAFETY (ключевой тест AGENT-5)**: МНОГО потоков конкурентно зовут `try_claim()` на
    /// бакете ёмкости N — суммарно успешных claim'ов РОВНО N (НЕ больше). Доказывает, что
    /// compare_exchange сериализует декремент: гонка check-then-act 3d (два диспетча оба видят
    /// `count<cap` и оба применяют) НЕВОЗМОЖНА. Без рефилла (часы не двигаем) — чистая ёмкость.
    #[test]
    fn concurrent_claims_never_exceed_capacity() {
        use std::sync::atomic::AtomicU32 as A32;
        const CAPACITY: u32 = 50;
        const THREADS: usize = 16;
        const PER_THREAD: usize = 20; // 16*20 = 320 попыток на 50 токенов.
        let clock = Arc::new(ManualClock::new()); // не двигаем → рефилла нет.
        let bucket = TokenBucket::with_clock(CAPACITY, 1, Duration::from_secs(60), clock);
        let claimed = Arc::new(A32::new(0));

        std::thread::scope(|s| {
            for _ in 0..THREADS {
                let bucket = bucket.clone();
                let claimed = claimed.clone();
                s.spawn(move || {
                    for _ in 0..PER_THREAD {
                        if bucket.try_claim() {
                            claimed.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });
            }
        });

        assert_eq!(
            claimed.load(Ordering::SeqCst),
            CAPACITY,
            "конкурентные claim'ы НЕ превышают ёмкость (РОВНО {CAPACITY})"
        );
        assert_eq!(bucket.available(), 0, "бакет пуст после ровно N claim'ов");
    }

    /// **CONCURRENCY-SAFETY рефилла**: конкурентные claim'ы ПОСЛЕ рефилл-окна не доначисляют дважды —
    /// `last_refill` продвигается CAS'ом (одно окно учитывается ровно раз). Опустошаем, продвигаем
    /// часы на 1 окно (+capacity токенов суммарно но ≤ capacity), затем конкурентно клеймим: суммарно
    /// успешных ≤ capacity (не capacity*threads из-за двойного начисления).
    #[test]
    fn concurrent_refill_no_double_credit() {
        use std::sync::atomic::AtomicU32 as A32;
        const CAPACITY: u32 = 8;
        let clock = Arc::new(ManualClock::new());
        // Рефилл сразу ВСЕЙ ёмкости за одно окно (10 с) — чтобы после опустошения одно окно вернуло
        // весь бакет; проверяем, что конкурентные claim'ы не «увидят» это окно несколько раз.
        let bucket =
            TokenBucket::with_clock(CAPACITY, CAPACITY, Duration::from_secs(10), clock.clone());
        // Опустошаем.
        for _ in 0..CAPACITY {
            assert!(bucket.try_claim());
        }
        assert!(!bucket.try_claim(), "пуст");
        // Одно окно прошло → доначислится CAPACITY (но не выше потолка).
        clock.advance(Duration::from_secs(10));

        let claimed = Arc::new(A32::new(0));
        std::thread::scope(|s| {
            for _ in 0..16 {
                let bucket = bucket.clone();
                let claimed = claimed.clone();
                s.spawn(move || {
                    for _ in 0..10 {
                        if bucket.try_claim() {
                            claimed.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });
            }
        });
        assert_eq!(
            claimed.load(Ordering::SeqCst),
            CAPACITY,
            "одно рефилл-окно вернуло РОВНО capacity — нет двойного начисления при гонке"
        );
    }

    // Доступ к reader sink'а для проверок ledger в тестах (зеркало apply.rs).
    fn sink_reader(sink: &AuditSink) -> crate::db::ReadPool {
        sink.reader_handle()
    }
}
