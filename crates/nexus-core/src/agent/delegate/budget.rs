//! `DelegationBudget` (SUB-0) — ОБЩИЙ между родителем и всеми субагентами анти-runaway бюджет дерева
//! делегирования. Клонируется в каждый дочерний `SessionSpec`; счётчик спавнов — за `Arc<AtomicUsize>`,
//! поэтому ОДИН пул на всё дерево прогона (fan-out не может его размножить). Глубина — per-уровень
//! (ребёнок получает `depth-1` через [`DelegationBudget::child_budget`]); на `max_depth=1` ребёнок имеет
//! `remaining_depth=0` → рекурсия структурно невозможна (второй чекпоинт поверх вырезания `delegate.run`
//! из реестра ребёнка — defense-in-depth).
//!
//! ВСЁ fail-closed: [`DelegationBudget::check_then_acquire_spawn`] проверяет глубину → дедлайн → атомарно
//! списывает спавн (CAS, без underflow); исчерпание ЛЮБОГО ресурса → `Err` ДО любого побочного эффекта.
//! Время дедлайна — `Instant` (монотонные часы; прод-вызов: `wall_clock` из конфига; тесты:
//! `Duration::ZERO` → дедлайн уже истёк детерминированно).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ai::DelegationConfig;

/// Причина отказа в спавне субагента (fail-closed). Все варианты → `delegate.run` вернёт recoverable
/// is_error tool-result, БЕЗ спавна (никаких частичных эффектов).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetError {
    /// Глубина дерева исчерпана (этот уровень не вправе порождать детей — рекурсия/слишком глубоко).
    DepthExhausted,
    /// Суммарный лимит спавнов дерева исчерпан (анти-runaway).
    SpawnsExhausted,
    /// Истёк общий дедлайн прогона (ребёнок не может пережить родителя).
    DeadlineExceeded,
}

impl std::fmt::Display for BudgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            BudgetError::DepthExhausted => "достигнута максимальная глубина делегирования",
            BudgetError::SpawnsExhausted => "исчерпан лимит субагентов за прогон",
            BudgetError::DeadlineExceeded => "истёк дедлайн прогона",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for BudgetError {}

/// Общий бюджет дерева делегирования. `Clone` дёшев: глубина копируется (per-уровень), счётчик спавнов
/// и дедлайн разделяются (`Arc`/`Copy`). Передаётся в каждый `SessionSpec`.
#[derive(Debug, Clone)]
pub struct DelegationBudget {
    /// Сколько ещё УРОВНЕЙ вглубь можно делегировать ИЗ этого бюджета. Per-уровень (НЕ Arc): ребёнок
    /// получает `depth-1`. 0 → этот уровень не вправе спавнить (рекурсия-стоп).
    remaining_depth: usize,
    /// ОБЩИЙ на всё дерево счётчик оставшихся спавнов (Arc → все клоны/дети тянут из одного пула).
    remaining_spawns: Arc<AtomicUsize>,
    /// Макс. детей за ОДИН вызов `delegate.run` (батч-кап; сам по себе не списывается — проверяется
    /// инструментом против размера батча).
    max_fanout_per_call: usize,
    /// Общий дедлайн прогона (монотонные часы). Спавн после него запрещён (ребёнок не переживёт родителя).
    deadline: Instant,
}

/// Потолок `wall_clock` (1 год) — защита от паники `Instant + Duration` на абсурдно больших значениях
/// (ревью SUB-0 MINOR). Эффективно «безлимит», но без переполнения монотонных часов.
const MAX_WALL_CLOCK: Duration = Duration::from_secs(365 * 86_400);

impl DelegationBudget {
    /// Корневой бюджет прогона: `max_depth` уровней, `max_total_spawns` спавнов на дерево,
    /// `max_fanout_per_call` детей за вызов, дедлайн = `now + wall_clock`.
    ///
    /// Капы НОРМАЛИЗУЮТСЯ к ≥1 (ревью SUB-0 MAJOR): бюджет существует только для ВКЛЮЧЁННОГО прогона, а
    /// `0` любого капа сделал бы `delegate.run` зарегистрированным, но вечно-падающим без диагностики
    /// (incoherent: «включено, но спавнить нельзя»). `0 → 1` + `warn`: «включено» всегда значит «можно
    /// хотя бы один». Выключают делегирование флагом `enabled=false`, НЕ нулевым капом. `wall_clock`
    /// клампится к [`MAX_WALL_CLOCK`] (анти-overflow `Instant + Duration`).
    pub fn new(
        max_depth: usize,
        max_total_spawns: usize,
        max_fanout_per_call: usize,
        wall_clock: Duration,
    ) -> Self {
        if max_depth == 0 || max_total_spawns == 0 || max_fanout_per_call == 0 {
            tracing::warn!(
                max_depth,
                max_total_spawns,
                max_fanout_per_call,
                "DelegationBudget: нулевой кап нормализован к 1 (делегирование выключают флагом enabled=false, не нулём)"
            );
        }
        let wall_clock = wall_clock.min(MAX_WALL_CLOCK);
        Self {
            remaining_depth: max_depth.max(1),
            remaining_spawns: Arc::new(AtomicUsize::new(max_total_spawns.max(1))),
            max_fanout_per_call: max_fanout_per_call.max(1),
            deadline: Instant::now() + wall_clock,
        }
    }

    /// Корневой бюджет из конфига делегирования + общий `wall_clock` прогона. (`enabled` здесь не
    /// смотрим — регистрацию инструмента гейтит вызывающий; бюджет — про КАПЫ.)
    pub fn from_config(cfg: &DelegationConfig, wall_clock: Duration) -> Self {
        Self::new(
            cfg.max_depth,
            cfg.max_total_spawns,
            cfg.max_fanout,
            wall_clock,
        )
    }

    /// Бюджет РЕБЁНКА: глубина-1 (saturating, без underflow), ОБЩИЙ счётчик спавнов и дедлайн. На
    /// `max_depth=1` ребёнок получает `remaining_depth=0` → сам спавнить не сможет (рекурсия-стоп).
    pub fn child_budget(&self) -> Self {
        Self {
            remaining_depth: self.remaining_depth.saturating_sub(1),
            remaining_spawns: Arc::clone(&self.remaining_spawns),
            max_fanout_per_call: self.max_fanout_per_call,
            deadline: self.deadline,
        }
    }

    /// Остаток глубины этого бюджета (наблюдаемость/гейт регистрации `delegate.run`).
    pub fn remaining_depth(&self) -> usize {
        self.remaining_depth
    }

    /// Остаток ОБЩЕГО счётчика спавнов (наблюдаемость).
    pub fn remaining_spawns(&self) -> usize {
        self.remaining_spawns.load(Ordering::Acquire)
    }

    /// Кап детей за один вызов `delegate.run`.
    pub fn max_fanout_per_call(&self) -> usize {
        self.max_fanout_per_call
    }

    /// Истёк ли общий дедлайн прогона.
    pub fn deadline_exceeded(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// **Fail-closed проверка-и-списание ОДНОГО спавна** (зовётся ПЕРЕД порождением каждого ребёнка).
    /// Порядок: глубина → дедлайн → атомарное списание спавна. ЛЮБОЙ исчерпанный ресурс → `Err` БЕЗ
    /// побочного эффекта (счётчик не уходит в минус — CAS-цикл, на 0 возвращает `Err`, не списывая).
    pub fn check_then_acquire_spawn(&self) -> Result<(), BudgetError> {
        if self.remaining_depth == 0 {
            return Err(BudgetError::DepthExhausted);
        }
        if self.deadline_exceeded() {
            return Err(BudgetError::DeadlineExceeded);
        }
        // Атомарное «проверь-и-спиши» без underflow: CAS-цикл, на 0 — отказ.
        let mut cur = self.remaining_spawns.load(Ordering::Acquire);
        loop {
            if cur == 0 {
                return Err(BudgetError::SpawnsExhausted);
            }
            match self.remaining_spawns.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => cur = actual,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIG: Duration = Duration::from_secs(3600);

    #[test]
    fn budget_acquire_decrements_and_fails_closed_at_zero() {
        let b = DelegationBudget::new(1, 1, 3, BIG);
        assert_eq!(b.remaining_spawns(), 1);
        assert!(b.check_then_acquire_spawn().is_ok(), "первый спавн ок");
        assert_eq!(b.remaining_spawns(), 0, "спавн списан");
        assert_eq!(
            b.check_then_acquire_spawn(),
            Err(BudgetError::SpawnsExhausted),
            "второй спавн отказан"
        );
        assert_eq!(
            b.remaining_spawns(),
            0,
            "без underflow (счётчик не ушёл в минус)"
        );

        // Глубина 0 (достигается ТОЛЬКО через child_budget на max_depth=1 — `new` клампит к ≥1):
        // отказ ещё до проверки спавнов, спавн НЕ списан.
        let root = DelegationBudget::new(1, 5, 3, BIG);
        let depth0 = root.child_budget();
        assert_eq!(depth0.remaining_depth(), 0);
        assert_eq!(
            depth0.check_then_acquire_spawn(),
            Err(BudgetError::DepthExhausted)
        );
        assert_eq!(
            depth0.remaining_spawns(),
            5,
            "спавн НЕ списан при depth=0 (общий пул не тронут)"
        );
    }

    /// Ревью SUB-0 MAJOR: нулевые капы из конфига НОРМАЛИЗУЮТСЯ к ≥1 (а не делают `delegate.run`
    /// вечно-падающим). «Включено» всегда значит «можно хотя бы один спавн».
    #[test]
    fn zero_caps_are_normalized_to_one() {
        let b = DelegationBudget::new(0, 0, 0, BIG);
        assert_eq!(b.remaining_depth(), 1, "depth 0→1");
        assert_eq!(b.remaining_spawns(), 1, "spawns 0→1");
        assert_eq!(b.max_fanout_per_call(), 1, "fanout 0→1");
        assert!(
            b.check_then_acquire_spawn().is_ok(),
            "нормализованный бюджет позволяет ≥1 спавн"
        );
    }

    #[test]
    fn child_budget_has_depth_minus_one_and_shares_spawn_counter() {
        let parent = DelegationBudget::new(2, 2, 3, BIG);
        let child = parent.child_budget();
        assert_eq!(parent.remaining_depth(), 2);
        assert_eq!(child.remaining_depth(), 1, "ребёнок: глубина-1");
        let grandchild = child.child_budget();
        assert_eq!(
            grandchild.remaining_depth(),
            0,
            "внук: глубина 0 (рекурсия-стоп)"
        );

        // ОБЩИЙ счётчик спавнов: родитель и ребёнок тянут из одного пула (всего 2).
        assert!(parent.check_then_acquire_spawn().is_ok()); // 2→1
        assert!(child.check_then_acquire_spawn().is_ok()); // 1→0 (тот же Arc)
        assert_eq!(parent.remaining_spawns(), 0);
        assert_eq!(
            child.remaining_spawns(),
            0,
            "счётчик разделяется между клонами"
        );
        assert_eq!(
            parent.check_then_acquire_spawn(),
            Err(BudgetError::SpawnsExhausted),
            "общий пул исчерпан обоими"
        );
    }

    #[test]
    fn budget_deadline_elapsed_is_reported() {
        // wall_clock=0 → дедлайн = now, к моменту проверки уже истёк (монотонные часы не идут назад).
        let b = DelegationBudget::new(5, 5, 3, Duration::ZERO);
        assert!(b.deadline_exceeded());
        assert_eq!(
            b.check_then_acquire_spawn(),
            Err(BudgetError::DeadlineExceeded),
            "истёкший дедлайн репортится РАНЬШЕ списания спавна"
        );
        assert_eq!(
            b.remaining_spawns(),
            5,
            "спавн НЕ списан при истёкшем дедлайне"
        );
    }

    #[test]
    fn from_config_uses_caps() {
        let cfg = DelegationConfig::default(); // depth=1, fanout=3, spawns=8
        let b = DelegationBudget::from_config(&cfg, BIG);
        assert_eq!(b.remaining_depth(), 1);
        assert_eq!(b.remaining_spawns(), 8);
        assert_eq!(b.max_fanout_per_call(), 3);
    }

    /// Ревью SUB-0 NIT (ключевой инвариант безопасности): под МНОГОПОТОЧНОЙ гонкой клонов суммарно
    /// успешных acquire РОВНО == кап (CAS не даёт превысить пул и не уходит в underflow). 32 потока ×
    /// 50 попыток против пула 100 → ровно 100 Ok, 1500 Err, остаток 0.
    #[test]
    fn concurrent_acquire_never_exceeds_cap() {
        use std::sync::atomic::AtomicUsize as Cnt;
        const THREADS: usize = 32;
        const PER_THREAD: usize = 50;
        const CAP: usize = 100;
        let budget = DelegationBudget::new(1, CAP, 3, BIG);
        let oks = Arc::new(Cnt::new(0));
        std::thread::scope(|s| {
            for _ in 0..THREADS {
                let b = budget.clone(); // общий Arc-счётчик спавнов
                let oks = Arc::clone(&oks);
                s.spawn(move || {
                    for _ in 0..PER_THREAD {
                        if b.check_then_acquire_spawn().is_ok() {
                            oks.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        assert_eq!(
            oks.load(Ordering::Relaxed),
            CAP,
            "ровно CAP успешных acquire под гонкой (без превышения)"
        );
        assert_eq!(
            budget.remaining_spawns(),
            0,
            "пул исчерпан ровно, без underflow"
        );
    }
}
