//! Слой актуатора (AGENT-3b, Фаза 1) — ЯДРО ЛОГИКИ + персистентность, БЕЗ побочных эффектов.
//!
//! Этот срез — ПУРЕ-логика + одна таблица БД, ноль записи на диск, ноль инструментов, ноль apply, ноль
//! enforcement автономии, ноль проводки в agentd. Безопасно лендить до vault-write-чекпойнта. Состав:
//! - [`action`] — типизированная алгебра [`Action`]/[`ActionTarget`]: fail-closed граница by-construction
//!   (shell/web/host-варианты НЕПРЕДСТАВИМЫ).
//! - [`classify`] — PURE fail-closed [`classify::classify`]: exhaustive по [`ActionTarget`] БЕЗ catch-all
//!   (keystone D4 «no catch-all-downgrade»).
//! - [`audit`] — idempotency-ledger (`agent_actions`, миграция 022): write-before-act API + replay по
//!   ПРИСУТСТВИЮ outcome (не ключа).
//! - этот модуль — типы статус-машины [`ActionState`] + ПУРЕ-валидация переходов + scaffold [`UndoHandle`].
//!
//! Исполнение/apply (запись в vault, snapshot, undo-наполнение), реальные инструменты, enforcement
//! автономии и проводка headless-демона — это AGENT-3c/3d/3e. ЗДЕСЬ их НЕТ намеренно.

pub mod action;
pub mod audit;
pub mod classify;

pub use action::{Action, ActionTarget};
pub use audit::{
    canonical_args, idempotency_key, replay_decision, ActionEntry, ActionRow, ReplayDecision,
    UndoCols,
};
pub use classify::{classify, BlockReason, ClassifyCtx, ConfirmReason, RiskTier};

/// Состояние действия в статус-машине актуатора (значения `agent_actions.state`).
///
/// Жизненный цикл (ребро = допустимый переход, см. [`ActionState::can_transition_to`]):
/// ```text
///   Classified ─┬─► Approved ──► Executing ─┬─► Executed ─► Audited
///               ├─► Proposed ──► Approved          └─► Failed ───► Audited
///               │           └──► Rejected ──► Audited
///               └─► Rejected ──► Audited
/// ```
/// - `Classified` — classify вынес тир; ещё не решено исполнять.
/// - `Proposed` — тир Confirm: показано пользователю, ждём апрув/реджект.
/// - `Approved` — разрешено к исполнению (auto-тир сразу, либо после апрува Proposed).
/// - `Rejected` — отклонено (HardBlocked или пользователь отказал) — в исполнение не пойдёт.
/// - `Executing` — apply начат (write-before-act записан) — AGENT-3c.
/// - `Executed` — apply успешен.
/// - `Failed` — apply упал.
/// - `Audited` — терминал: исход зафиксирован в журнале (поглощающий, исходящих рёбер нет).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionState {
    Classified,
    Proposed,
    Approved,
    Rejected,
    Executing,
    Executed,
    Failed,
    Audited,
}

impl ActionState {
    /// Стабильный строковый дискриминант (значение `agent_actions.state`) — единый источник со
    /// строковыми константами в [`audit`]. SQL/чтения/переходы не разъезжаются по опечаткам.
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionState::Classified => "classified",
            ActionState::Proposed => "proposed",
            ActionState::Approved => "approved",
            ActionState::Rejected => "rejected",
            ActionState::Executing => "executing",
            ActionState::Executed => "executed",
            ActionState::Failed => "failed",
            ActionState::Audited => "audited",
        }
    }

    /// Парс из строкового дискриминанта (обратное к [`as_str`]). `None` — неизвестное значение.
    /// Тонкая обёртка над [`std::str::FromStr`] для удобства (`Option` вместо `Result`).
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// Терминально ли состояние (поглощающее — исходящих переходов нет). Только `Audited`.
    pub fn is_terminal(&self) -> bool {
        matches!(self, ActionState::Audited)
    }

    /// ПУРЕ-валидация перехода: разрешён ли `self → next`. Без IO. EXHAUSTIVE по `self` (НЕТ `_ =>`):
    /// каждое состояние перечисляет СВОИ допустимые цели явно — новое состояние заставит дописать ветку
    /// (тот же D4-инвариант, что и в classify: ничего не разрешается «по умолчанию»).
    pub fn can_transition_to(&self, next: ActionState) -> bool {
        use ActionState::*;
        match self {
            // После classify: авто → Approved; confirm → Proposed; hardblocked/отказ → Rejected.
            Classified => matches!(next, Approved | Proposed | Rejected),
            // Показано юзеру: апрув → Approved, отказ → Rejected.
            Proposed => matches!(next, Approved | Rejected),
            // Разрешено: уходит в исполнение.
            Approved => matches!(next, Executing),
            // Отклонено: только в аудит (терминальная фиксация).
            Rejected => matches!(next, Audited),
            // Исполняется: успех → Executed, провал → Failed.
            Executing => matches!(next, Executed | Failed),
            // Успех/провал: в аудит.
            Executed => matches!(next, Audited),
            Failed => matches!(next, Audited),
            // Терминал: исходящих переходов нет.
            Audited => false,
        }
    }
}

impl std::str::FromStr for ActionState {
    type Err = ();

    /// Парс из строкового дискриминанта (обратное к [`ActionState::as_str`]). `Err(())` — неизвестное
    /// значение. EXHAUSTIVE отображение строк ↔ вариантов — единый источник со SQL `agent_actions.state`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "classified" => ActionState::Classified,
            "proposed" => ActionState::Proposed,
            "approved" => ActionState::Approved,
            "rejected" => ActionState::Rejected,
            "executing" => ActionState::Executing,
            "executed" => ActionState::Executed,
            "failed" => ActionState::Failed,
            "audited" => ActionState::Audited,
            _ => return Err(()),
        })
    }
}

/// Хэндл отмены действия (AGENT-4 consumes; AGENT-3c populates; ЗДЕСЬ — только тип-scaffold).
///
/// Дискриминант + ссылка сериализуются в ledger (`agent_actions.undo_kind`/`undo_ref`) через [`UndoCols`].
/// Это scaffold: способа СОЗДАТЬ реальный snapshot/trash в этом срезе нет (apply — 3c). Зеркало
/// (kind,ref) ↔ вариант держим в [`UndoHandle::to_cols`]/[`UndoHandle::from_cols`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoHandle {
    /// Снимок прежнего содержимого заметки `rel` на отметке `ts` (unix-сек) — откат NoteEdit/Frontmatter.
    Snapshot { rel: String, ts: i64 },
    /// Файл перенесён в vault-корзину по `trash_rel` — откат удаления/перезаписи через восстановление.
    Trash { trash_rel: String },
}

/// Дискриминанты [`UndoHandle`] для ledger — единый источник строк.
pub const UNDO_SNAPSHOT: &str = "snapshot";
pub const UNDO_TRASH: &str = "trash";

impl UndoHandle {
    /// Сериализация в (kind, ref) для хранения в ledger. `ref` для Snapshot — `ts` (строкой), для Trash —
    /// `trash_rel`. (rel у Snapshot хранится отдельно как `target_rel` строки действия — не дублируем.)
    pub fn to_cols(&self) -> UndoCols {
        match self {
            UndoHandle::Snapshot { ts, .. } => UndoCols {
                kind: UNDO_SNAPSHOT.to_string(),
                reference: ts.to_string(),
            },
            UndoHandle::Trash { trash_rel } => UndoCols {
                kind: UNDO_TRASH.to_string(),
                reference: trash_rel.clone(),
            },
        }
    }

    /// Десериализация из ledger-колонок. `rel` нужен для восстановления Snapshot (берётся из
    /// `target_rel` строки действия — у Trash игнорируется). `None` — неизвестный kind или битый ref.
    pub fn from_cols(kind: &str, reference: &str, rel: &str) -> Option<Self> {
        match kind {
            UNDO_SNAPSHOT => reference
                .parse::<i64>()
                .ok()
                .map(|ts| UndoHandle::Snapshot {
                    rel: rel.to_string(),
                    ts,
                }),
            UNDO_TRASH => Some(UndoHandle::Trash {
                trash_rel: reference.to_string(),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// as_str ↔ from_str round-trip для всех состояний (единый источник со строками ledger).
    #[test]
    fn action_state_str_roundtrip() {
        for s in [
            ActionState::Classified,
            ActionState::Proposed,
            ActionState::Approved,
            ActionState::Rejected,
            ActionState::Executing,
            ActionState::Executed,
            ActionState::Failed,
            ActionState::Audited,
        ] {
            assert_eq!(ActionState::parse(s.as_str()), Some(s));
            assert_eq!(s.as_str().parse::<ActionState>(), Ok(s));
        }
        assert_eq!(ActionState::parse("bogus"), None);
        assert_eq!("bogus".parse::<ActionState>(), Err(()));
    }

    /// Валидные переходы happy-path (auto): Classified→Approved→Executing→Executed→Audited.
    #[test]
    fn valid_auto_path() {
        use ActionState::*;
        assert!(Classified.can_transition_to(Approved));
        assert!(Approved.can_transition_to(Executing));
        assert!(Executing.can_transition_to(Executed));
        assert!(Executed.can_transition_to(Audited));
    }

    /// Валидный confirm-path: Classified→Proposed→Approved→Executing→Failed→Audited.
    #[test]
    fn valid_confirm_and_fail_path() {
        use ActionState::*;
        assert!(Classified.can_transition_to(Proposed));
        assert!(Proposed.can_transition_to(Approved));
        assert!(Proposed.can_transition_to(Rejected));
        assert!(Executing.can_transition_to(Failed));
        assert!(Failed.can_transition_to(Audited));
        assert!(Rejected.can_transition_to(Audited));
    }

    /// Невалидные переходы ОТВЕРГАЮТСЯ — в т.ч. «воскрешение» терминала и пропуск стадий.
    #[test]
    fn invalid_transitions_rejected() {
        use ActionState::*;
        // Терминал не воскресает.
        assert!(!Audited.can_transition_to(Executing));
        assert!(!Audited.can_transition_to(Classified));
        // Назад по машине.
        assert!(!Executed.can_transition_to(Classified));
        assert!(!Executing.can_transition_to(Classified));
        // Пропуск стадий (нельзя из Classified сразу исполнять).
        assert!(!Classified.can_transition_to(Executing));
        assert!(!Classified.can_transition_to(Executed));
        // Approved не отклоняется (решение уже принято) и не финишируется напрямую.
        assert!(!Approved.can_transition_to(Rejected));
        assert!(!Approved.can_transition_to(Audited));
        // Rejected не исполняется.
        assert!(!Rejected.can_transition_to(Executing));
        assert!(!Rejected.can_transition_to(Approved));
    }

    /// Только Audited терминален.
    #[test]
    fn only_audited_is_terminal() {
        assert!(ActionState::Audited.is_terminal());
        for s in [
            ActionState::Classified,
            ActionState::Proposed,
            ActionState::Approved,
            ActionState::Rejected,
            ActionState::Executing,
            ActionState::Executed,
            ActionState::Failed,
        ] {
            assert!(!s.is_terminal(), "{} не должен быть терминалом", s.as_str());
        }
    }

    /// UndoHandle round-trip через ledger-колонки (scaffold AGENT-4).
    #[test]
    fn undo_handle_cols_roundtrip() {
        let snap = UndoHandle::Snapshot {
            rel: "Notes/N.md".to_string(),
            ts: 1_700_000_000,
        };
        let cols = snap.to_cols();
        assert_eq!(cols.kind, UNDO_SNAPSHOT);
        assert_eq!(cols.reference, "1700000000");
        assert_eq!(
            UndoHandle::from_cols(&cols.kind, &cols.reference, "Notes/N.md"),
            Some(snap)
        );

        let trash = UndoHandle::Trash {
            trash_rel: ".nexus/.trash/123-N.md".to_string(),
        };
        let cols = trash.to_cols();
        assert_eq!(cols.kind, UNDO_TRASH);
        assert_eq!(
            UndoHandle::from_cols(&cols.kind, &cols.reference, "ignored"),
            Some(trash)
        );

        // Битый kind / битый snapshot-ref → None.
        assert_eq!(UndoHandle::from_cols("bogus", "x", "r"), None);
        assert_eq!(UndoHandle::from_cols(UNDO_SNAPSHOT, "not-a-ts", "r"), None);
    }
}
