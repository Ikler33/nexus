//! Слой актуатора (AGENT-3b/3c, Фаза 1) — ЯДРО ЛОГИКИ + персистентность + host-side APPLY/инструменты.
//!
//! 3b-фундамент (пуре-логика + одна таблица БД) + 3c-исполнение (запись в vault за всеми рубежами).
//! ЗАПИСЬ НА ДИСК происходит ТОЛЬКО в [`apply`] (и только через временные vault'ы тестов — живой проводки
//! нет). Состав:
//! - [`action`] — типизированная алгебра [`Action`]/[`ActionTarget`]: fail-closed граница by-construction
//!   (shell/web/host-варианты НЕПРЕДСТАВИМЫ).
//! - [`classify`] — PURE fail-closed [`classify::classify`]: exhaustive по [`ActionTarget`] БЕЗ catch-all
//!   (keystone D4 «no catch-all-downgrade»).
//! - [`audit`] — idempotency-ledger (`agent_actions`, миграция 022): write-before-act API + replay по
//!   ПРИСУТСТВИЮ outcome (не ключа).
//! - этот модуль — типы статус-машины [`ActionState`] + ПУРЕ-валидация переходов + [`UndoHandle`].
//! - [`apply`] (3c) — [`apply::apply_action`]: host-side исполнитель за всеми рубежами (canonicalize/
//!   symlink rampart → drift → ledger write-before-act → snapshot manual=true → atomic_write → finish)
//!   + [`apply::AuditSink`] (обёртка ledger).
//! - [`tools`] (3c/3e) — файловые инструменты [`tools::NoteCreateTool`]/[`tools::NoteEditTool`]/
//!   [`tools::SetFrontmatterTool`] (impl [`crate::tool_types::Tool`]): `invoke` маршрутизирует ТОЛЬКО через
//!   гейт [`orchestrate::dispatch_action`] (3e hard-gate #1 — ungated direct-apply путь УДАЛЁН).
//! - [`decision`] (3d) — [`decision::DecisionSource`] (fail-closed [`decision::PolicyDefault`] +
//!   [`decision::ChannelDecision`]) + [`decision::ProposalBatch`]/[`decision::BatchDecision`]
//!   (отсутствующий айтем = Reject).
//! - [`orchestrate`] (3d) — гейт автономии [`orchestrate::dispatch_action`]: матрица `(RiskTier ×
//!   autonomy)`, эмиссия Proposal/Diff ([`orchestrate::EventSink`]), blast-radius-кэп, propose→decide→
//!   apply/reject ledger-флоу; `classify_hash` ОБЯЗАТЕЛЕН на пути apply; `overwrite_threshold` ИЗ КОНФИГА.
//!
//! ЖИВАЯ ПРОВОДКА (AGENT-3e): [`crate::agent::AgentRunHandler`] строит реестр гейтнутых инструментов
//! ПО-ПРОГОННО — но ТОЛЬКО когда конфиг-флаг `agent_actuator_enabled` ВКЛ (по умолчанию ВЫКЛ → стабы,
//! реальный vault не затронут из коробки). Headless agentd собирает их с [`decision::PolicyDefault`]
//! (auto-DENY). polный token-bucket — AGENT-5.

pub mod action;
pub mod apply;
pub mod audit;
pub mod classify;
pub mod decision;
pub mod orchestrate;
pub mod tools;
pub mod undo;

pub use action::{Action, ActionTarget};
// `apply_action` НЕ реэкспортируется (AGENT-3e Fix-2): его видимость сужена до
// `pub(in crate::actuator)` (no-bypass компайл-тайм). Реэкспорт `pub` поднял бы её обратно до
// крейт-публичной — вернув ungated direct-apply путь. Снаружи актуатора применение идёт ТОЛЬКО
// через `orchestrate::dispatch_action` (гейт автономии). Здесь реэкспортируем лишь типы исхода/леджера.
pub use apply::{ApplyOutcome, AuditSink};
pub use audit::{
    actions_for_undo, canonical_args, idempotency_key, mark_undone, reconcile_stale_executing,
    replay_decision, transition, ActionEntry, ActionRow, ReplayDecision, UndoCols,
    EXEC_STALE_TTL_SECS, STATE_UNDONE,
};
pub use classify::{classify, BlockReason, ClassifyCtx, ConfirmReason, RiskTier};
pub use decision::{
    ApproveAll, BatchDecision, ChannelDecision, DecisionSource, ItemDecision, PolicyDefault,
    ProposalBatch, ProposalItem,
};
#[cfg(any(test, feature = "test-util"))]
pub use orchestrate::ManualClock;
pub use orchestrate::{
    dispatch_action, dispatch_exec_decision, Clock, CollectingSink, DispatchOutcome,
    DispatchPolicy, EventSink, ExecDecision, MonotonicClock, TokenBucket, TracingEventSink,
    DEFAULT_REFILL_PER, DEFAULT_REFILL_TOKENS,
};
pub use tools::{
    ActionDispatcher, GatedToolCtx, NoteCreateTool, NoteEditTool, SetFrontmatterTool, SkillSaveCtx,
    SkillSaveTool, OVERWRITE_THRESHOLD,
};
pub use undo::{undo_run, ActionUndo, UndoExecDriver, UndoOpts, UndoOutcome, UndoStatus};

/// Состояние действия в статус-машине актуатора (значения `agent_actions.state`).
///
/// Жизненный цикл (ребро = допустимый переход, см. [`ActionState::can_transition_to`]):
/// ```text
///   Classified ─┬─► Approved ──► Executing ─┬─► Executed ─┬─► Audited
///               ├─► Proposed ──► Approved          │       └─► Undone (AGENT-4: откат executed)
///               │           └──► Rejected ──► Audited └─► Failed ──► Audited
///               └─► Rejected ──► Audited
/// ```
/// - `Classified` — classify вынес тир; ещё не решено исполнять.
/// - `Proposed` — тир Confirm: показано пользователю, ждём апрув/реджект.
/// - `Approved` — разрешено к исполнению (auto-тир сразу, либо после апрува Proposed).
/// - `Rejected` — отклонено (HardBlocked или пользователь отказал) — в исполнение не пойдёт.
/// - `Executing` — apply начат (write-before-act записан) — AGENT-3c.
/// - `Executed` — apply успешен.
/// - `Failed` — apply упал.
/// - `Undone` — успешное действие ОТКАЧЕНО (AGENT-4): снапшот восстановлен / created-файл в корзине;
///   единственный путь сюда — `executed → undone` (откат необратим иначе). Помечает строку, чтобы
///   повторный `undo_run` её ПРОПУСТИЛ (идемпотентность отката). НЕ терминал статус-машины: остаётся
///   аудируемой записью (исход уже зафиксирован finish'ем при executed — undo лишь меняет state).
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
    Undone,
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
            ActionState::Undone => "undone",
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
            // Успех: в аудит ЛИБО откат (AGENT-4: executed → undone — единственное новое ребро).
            Executed => matches!(next, Audited | Undone),
            Failed => matches!(next, Audited),
            // Откачено: в аудит (фиксация подотчётности). Повторно откатить нечего.
            Undone => matches!(next, Audited),
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
            "undone" => ActionState::Undone,
            "audited" => ActionState::Audited,
            _ => return Err(()),
        })
    }
}

/// Хэндл отмены действия (AGENT-4 consumes; AGENT-3c [`apply`] populates).
///
/// Дискриминант + ссылка сериализуются в ledger (`agent_actions.undo_kind`/`undo_ref`) через [`UndoCols`].
/// 3c [`apply::apply_action`] эмитит КОРРЕКТНЫЙ хэндл: Snapshot{rel,ts} (откат overwrite — restore точки)
/// для NoteEdit/Frontmatter, Trash{trash_rel} (откат create — move_to_trash) для NoteCreate. Зеркало
/// (kind,ref) ↔ вариант держим в [`UndoHandle::to_cols`]/[`UndoHandle::from_cols`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoHandle {
    /// Снимок прежнего содержимого заметки `rel` на отметке `ts` (unix-МС — имя файла снапшота, см.
    /// [`crate::vault::history`]) — откат NoteEdit/Frontmatter через restore этой точки.
    Snapshot { rel: String, ts: i64 },
    /// Файл перенесён в vault-корзину по `trash_rel` — откат удаления/перезаписи через восстановление.
    Trash { trash_rel: String },
    /// Pre-op git-ref (sha ДО мутирующей exec-GitOp, §5.5 SANDBOX-6c-2h): `reference` — sha, к которому
    /// откатывается репозиторий. **Surfacing-only в 6c-2h**: [`super::undo::undo_run`] показывает ref, но
    /// РЕАЛЬНЫЙ `git reset --hard <ref>` (доп. in-container exec под host-апрувом) отложен в 6c-3 (Tier-2
    /// live, документ-seam). shell/process НЕОБРАТИМЫ (у них нет undo-хэндла вовсе).
    ExecGitRef { reference: String },
}

/// Дискриминанты [`UndoHandle`] для ledger — единый источник строк.
pub const UNDO_SNAPSHOT: &str = "snapshot";
pub const UNDO_TRASH: &str = "trash";
/// Дискриминант отмены exec-GitOp (Фаза-3 SANDBOX-6c §5.5): `undo_ref` несёт pre-op git-ref (sha до
/// мутирующей операции) для восстановления. Стабильная ledger-строка объявлена здесь (6c-2g); персист
/// `undo_ref→UndoCols{kind:exec_gitref}` в report и exec-ветка `from_cols`/`undo` — 6c-2h (GitOp pre-op-ref).
/// shell.run/process.spawn НЕОБРАТИМЫ (undo_ref=None) и НИКОГДА не Auto (classify).
pub const UNDO_EXEC_GITREF: &str = "exec_gitref";

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
            UndoHandle::ExecGitRef { reference } => UndoCols {
                kind: UNDO_EXEC_GITREF.to_string(),
                reference: reference.clone(),
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
            UNDO_EXEC_GITREF => Some(UndoHandle::ExecGitRef {
                reference: reference.to_string(),
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
            ActionState::Undone,
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
        // AGENT-4: откатить можно ТОЛЬКО успешное (executed → undone); прочие исходные → нет.
        assert!(
            !Failed.can_transition_to(Undone),
            "провал откатывать нечего"
        );
        assert!(
            !Executing.can_transition_to(Undone),
            "ещё не исполнено — нечего откатывать"
        );
        assert!(
            !Undone.can_transition_to(Executed),
            "откат не «воскрешает» действие"
        );
        assert!(
            !Undone.can_transition_to(Undone),
            "повторный откат уже откаченного запрещён (идемпотентность на уровне машины)"
        );
    }

    /// AGENT-4: единственное новое ребро отката — `executed → undone`, далее `undone → audited`.
    #[test]
    fn valid_undo_path() {
        use ActionState::*;
        assert!(
            Executed.can_transition_to(Undone),
            "успешное действие можно откатить"
        );
        assert!(
            Executed.can_transition_to(Audited),
            "executed по-прежнему может уйти в аудит (откат не обязателен)"
        );
        assert!(
            Undone.can_transition_to(Audited),
            "откаченное фиксируется в аудит"
        );
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
            ActionState::Undone,
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

    /// 6c-2h: ExecGitRef ↔ UndoCols round-trip (exec_gitref дискриминант, ref=sha; rel игнорируется).
    #[test]
    fn exec_gitref_cols_roundtrip() {
        let h = UndoHandle::ExecGitRef {
            reference: "deadbeefcafe".to_string(),
        };
        let cols = h.to_cols();
        assert_eq!(cols.kind, UNDO_EXEC_GITREF);
        assert_eq!(cols.reference, "deadbeefcafe");
        assert_eq!(
            UndoHandle::from_cols(UNDO_EXEC_GITREF, "deadbeefcafe", "ignored-rel"),
            Some(h)
        );
    }
}
