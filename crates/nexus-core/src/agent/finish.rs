//! R-2: КАНОНИЧЕСКИЙ маппинг [`LoopOutcome`] → терминал прогона run_store — единственный источник
//! статусов/текстов финализации (REFACTOR-PLAN §3, thermo-смелл №13). До дедупа маппинг был скопирован
//! ×5 (desktop `finish_in_store` / connect-handler / ACP-сервер / agentd-джоба (инлайн) / CLI) и уже
//! дрейфовал: CLI терял `Paused`-арм и врал «бюджет исчерпан (Paused)» (B13).
//!
//! Различия вызывателей НЕ унифицированы молча — они сохранены ЯВНЫМИ параметрами (строго
//! behavior-preserving, тексты попадают в run_store/историю прогонов/UI):
//! - [`PausePolicy`] — судьба паузы (kill-switch, `BudgetKind::Paused`): one-shot пути
//!   (desktop/connect/ACP/CLI) финализируют терминальный `error` с честным текстом; agentd-джоба
//!   ПАРКУЕТ прогон (→ [`RunFinish::Park`]: `requeue_to_queued` + пере-кью, возобновление на un-pause).
//! - [`CancelWording`] — pre-existing расхождение Cancelled-текста: «прогон отменён» (desktop/connect/
//!   ACP/agentd) vs историческое CLI-шное «отменён» — сохранено per-caller, НЕ починено молча.
//!
//! Специфичные НЕ-run_store проекции (например, ACP `stopReason`) остаются тонкими функциями у своих
//! вызывателей — «прямое > магического»: канон несёт ОБЩЕЕ ядро, а не франкен-тип всех форм сразу.

use super::run_store::{STATUS_CANCELLED, STATUS_DONE, STATUS_ERROR};
use super::runner::{BudgetKind, LoopOutcome};

/// Политика обращения с паузой (kill-switch, [`BudgetKind::Paused`]) при финализации исхода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PausePolicy {
    /// Пауза — терминальный `error` с честным текстом «прогон приостановлен (kill-switch)»: one-shot
    /// пути (desktop-spawn / connect-handler / ACP / CLI) без scheduler-пути возобновления.
    FinalizeError,
    /// Пауза — НЕ терминал → [`RunFinish::Park`]: вызывающий паркует прогон (agentd-джоба:
    /// `requeue_to_queued` + пере-кью на un-pause; AGENT-5 чек-пойнт #2). `finish_run` НЕ пишется.
    Requeue,
}

/// Формулировка Cancelled-текста. Pre-existing расхождение вызывателей сохранено параметром —
/// R-2 строго behavior-preserving (тексты уходят в run_store/историю/UI, молча не меняем).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelWording {
    /// «прогон отменён; частичный ответ: …» — desktop / connect-handler / ACP / agentd-джоба.
    RunCancelled,
    /// «отменён; частичный ответ: …» — nexus-cli (исторический текст one-shot CLI, сохранён как есть).
    CancelledBare,
}

/// Решение финализации прогона по исходу цикла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunFinish {
    /// Терминал: `finish_run(status, text)`; `status` — константа `run_store::STATUS_*`
    /// (`done`/`cancelled`/`error`).
    Finalize { status: &'static str, text: String },
    /// Пауза при [`PausePolicy::Requeue`]: прогон НЕ финализируется — вызывающий возвращает его в
    /// `queued` и пере-кьюит джобу (возобновление на un-pause).
    Park,
}

impl RunFinish {
    /// Терминальная пара `(status, text)` для one-shot путей: при [`PausePolicy::FinalizeError`] канон
    /// НИКОГДА не возвращает [`RunFinish::Park`] (доказано юнит-таблицей модуля) — паника здесь означает
    /// ошибку композиции вызывателя (политика [`PausePolicy::Requeue`] обязана матчить `Park` явно).
    #[track_caller]
    pub fn expect_finalize(self) -> (&'static str, String) {
        match self {
            RunFinish::Finalize { status, text } => (status, text),
            RunFinish::Park => {
                panic!("RunFinish::Park у терминального вызывателя (ожидался Finalize)")
            }
        }
    }
}

/// Канонический маппинг исхода цикла → решение финализации прогона.
///
/// Единственное место, где живут статусы И тексты терминала run_store (Final → `done`; Cancelled →
/// `cancelled` с текстом по [`CancelWording`]; Paused → по [`PausePolicy`]; прочее исчерпание бюджета
/// (Steps/WallClock/Tokens) → `error` «бюджет исчерпан ({kind:?}); …»; Error → `error` с текстом
/// ошибки как есть). `partial`-хвост «; частичный ответ: …» сохраняется во всех бюджет-армах —
/// UI показывает хоть что-то.
pub fn outcome_to_finish(
    outcome: &LoopOutcome,
    policy: PausePolicy,
    cancel: CancelWording,
) -> RunFinish {
    match outcome {
        LoopOutcome::Final(s) => RunFinish::Finalize {
            status: STATUS_DONE,
            text: s.clone(),
        },
        LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Cancelled,
            partial,
        } => RunFinish::Finalize {
            status: STATUS_CANCELLED,
            text: match cancel {
                CancelWording::RunCancelled => {
                    format!("прогон отменён; частичный ответ: {partial}")
                }
                CancelWording::CancelledBare => format!("отменён; частичный ответ: {partial}"),
            },
        },
        LoopOutcome::BudgetExhausted {
            kind: BudgetKind::Paused,
            partial,
        } => match policy {
            PausePolicy::FinalizeError => RunFinish::Finalize {
                status: STATUS_ERROR,
                text: format!("прогон приостановлен (kill-switch); частичный ответ: {partial}"),
            },
            PausePolicy::Requeue => RunFinish::Park,
        },
        LoopOutcome::BudgetExhausted { kind, partial } => RunFinish::Finalize {
            status: STATUS_ERROR,
            text: format!("бюджет исчерпан ({kind:?}); частичный ответ: {partial}"),
        },
        LoopOutcome::Error(e) => RunFinish::Finalize {
            status: STATUS_ERROR,
            text: e.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(kind: BudgetKind) -> LoopOutcome {
        LoopOutcome::BudgetExhausted {
            kind,
            partial: "часть".into(),
        }
    }

    /// Общие для ОБЕИХ политик/формулировок армы (Paused и Cancelled характеризуются отдельно).
    fn common_rows() -> [(LoopOutcome, &'static str, &'static str); 5] {
        [
            (LoopOutcome::Final("итог".into()), STATUS_DONE, "итог"),
            (
                be(BudgetKind::Steps),
                STATUS_ERROR,
                "бюджет исчерпан (Steps); частичный ответ: часть",
            ),
            (
                be(BudgetKind::WallClock),
                STATUS_ERROR,
                "бюджет исчерпан (WallClock); частичный ответ: часть",
            ),
            (
                be(BudgetKind::Tokens),
                STATUS_ERROR,
                "бюджет исчерпан (Tokens); частичный ответ: часть",
            ),
            (LoopOutcome::Error("упал".into()), STATUS_ERROR, "упал"),
        ]
    }

    /// ПОЛНАЯ таблица: варианты × политики × формулировки. Final/Steps/WallClock/Tokens/Error
    /// не зависят ни от политики, ни от формулировки (байт-в-байт фикстура R-2).
    #[test]
    fn full_table_common_arms_invariant_to_policy_and_wording() {
        for policy in [PausePolicy::FinalizeError, PausePolicy::Requeue] {
            for wording in [CancelWording::RunCancelled, CancelWording::CancelledBare] {
                for (outcome, want_status, want_text) in common_rows() {
                    let got = outcome_to_finish(&outcome, policy, wording);
                    assert_eq!(
                        got,
                        RunFinish::Finalize {
                            status: want_status,
                            text: want_text.into()
                        },
                        "вариант: {outcome:?}, политика: {policy:?}, формулировка: {wording:?}"
                    );
                }
            }
        }
    }

    /// Cancelled: формулировка выбирается ТОЛЬКО параметром [`CancelWording`] (политика не влияет),
    /// статус всегда `cancelled` — pre-existing расхождение текстов зафиксировано, не «починено».
    #[test]
    fn cancelled_wording_is_explicit_parameter() {
        for policy in [PausePolicy::FinalizeError, PausePolicy::Requeue] {
            assert_eq!(
                outcome_to_finish(
                    &be(BudgetKind::Cancelled),
                    policy,
                    CancelWording::RunCancelled
                ),
                RunFinish::Finalize {
                    status: STATUS_CANCELLED,
                    text: "прогон отменён; частичный ответ: часть".into()
                },
                "политика: {policy:?}"
            );
            assert_eq!(
                outcome_to_finish(
                    &be(BudgetKind::Cancelled),
                    policy,
                    CancelWording::CancelledBare
                ),
                RunFinish::Finalize {
                    status: STATUS_CANCELLED,
                    text: "отменён; частичный ответ: часть".into()
                },
                "политика: {policy:?}"
            );
        }
    }

    /// Paused: судьба решается ТОЛЬКО политикой — FinalizeError → терминальный `error` с честным
    /// текстом (формулировка Cancelled не влияет); Requeue → [`RunFinish::Park`] (парковка agentd).
    #[test]
    fn paused_fate_is_policy_only() {
        for wording in [CancelWording::RunCancelled, CancelWording::CancelledBare] {
            assert_eq!(
                outcome_to_finish(&be(BudgetKind::Paused), PausePolicy::FinalizeError, wording),
                RunFinish::Finalize {
                    status: STATUS_ERROR,
                    text: "прогон приостановлен (kill-switch); частичный ответ: часть".into()
                },
                "формулировка: {wording:?}"
            );
            assert_eq!(
                outcome_to_finish(&be(BudgetKind::Paused), PausePolicy::Requeue, wording),
                RunFinish::Park,
                "формулировка: {wording:?}"
            );
        }
    }

    /// Инвариант для one-shot вызывателей: FinalizeError НИКОГДА не даёт Park — `expect_finalize`
    /// безопасен по построению (полный перебор вариантов, обе формулировки).
    #[test]
    fn finalize_error_never_parks() {
        let all = [
            LoopOutcome::Final("итог".into()),
            be(BudgetKind::Steps),
            be(BudgetKind::WallClock),
            be(BudgetKind::Tokens),
            be(BudgetKind::Cancelled),
            be(BudgetKind::Paused),
            LoopOutcome::Error("упал".into()),
        ];
        for outcome in all {
            for wording in [CancelWording::RunCancelled, CancelWording::CancelledBare] {
                let got = outcome_to_finish(&outcome, PausePolicy::FinalizeError, wording);
                assert!(
                    matches!(got, RunFinish::Finalize { .. }),
                    "вариант: {outcome:?}, формулировка: {wording:?}"
                );
                // и разворачивается без паники
                let _ = outcome_to_finish(&outcome, PausePolicy::FinalizeError, wording)
                    .expect_finalize();
            }
        }
    }

    /// `expect_finalize` на Park — паника (ошибка композиции): Requeue-вызыватель обязан матчить Park.
    #[test]
    #[should_panic(expected = "RunFinish::Park")]
    fn expect_finalize_panics_on_park() {
        let _ = outcome_to_finish(
            &be(BudgetKind::Paused),
            PausePolicy::Requeue,
            CancelWording::RunCancelled,
        )
        .expect_finalize();
    }
}
