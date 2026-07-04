//! Конвейер propose→decide→transition гейта актуатора — применение (`apply_now`) + КАНОН пропоуз-раунда
//! ([`run_proposal_round`]) + три тонкие обёртки-вызывателя (`propose_and_decide` vault-changeset,
//! [`dispatch_skill_save`] навык, [`dispatch_exec_decision`] exec).
//!
//! R-5c (REFACTOR-PLAN, thermo-смелл №5, финал стадии R-5): три копии конвейера «propose→decide→transition»
//! сведены к ОДНОМУ канону [`run_proposal_round`] (record_before-proposed+dup-lookup → события → decide →
//! **kill-switch re-check ПОСЛЕ decide ПЕРЕД transition** → transition), а сам конвейер вынесен из
//! `orchestrate.rs` в этот подмодуль (byte-identical; после R-5a/b `orchestrate.rs` уже под целью <1000).
//! Публичные имена (`dispatch_exec_decision`/`ExecDecision`/`dispatch_skill_save`) реэкспортируются из
//! `orchestrate` без изменения внешних путей; общие хелперы гейта (`block_message`/`proposed_content`/…)
//! и `DispatchPolicy`/`DispatchOutcome` берутся из родителя (`use super::…`).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::event::{AgentEvent, ProposedFile};
use crate::tool_types::ToolError;

use crate::actuator::action::{Action, ActionTarget};
use crate::actuator::apply::{
    apply_action, apply_skill_save, confine_for_overwrite, ApplyOutcome, AuditSink,
};
use crate::actuator::audit::{
    self, canonical_args, idempotency_key, ActionEntry, DiffSummary, STATE_APPROVED,
    STATE_PROPOSED, STATE_REJECTED,
};
use crate::actuator::classify::{classify, ClassifyCtx, RiskTier};
use crate::actuator::decision::{DecisionSource, ItemDecision, ProposalBatch, ProposalItem};

// Общие хелперы гейта + политика/исход из родительского `orchestrate` (приватные — доступны подмодулю).
use super::{
    block_message, change_kind, file_status, line_diff, proposed_content, DispatchOutcome,
    DispatchPolicy, EventSink,
};

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
pub(crate) async fn apply_now(
    action: &Action,
    run_id: i64,
    canon_root: &Path,
    ledger: &AuditSink,
    classify_hash: &str,
    agent_paused: &Arc<AtomicBool>,
) -> DispatchOutcome {
    // Фаза-3 defense-in-depth: exec-таргет СТРУКТУРНО не доходит сюда (classify_exec → Confirm/HardBlocked,
    // НИКОГДА Auto; а apply_now — только Auto-путь). Но fail-closed на случай будущего рефактора: exec НЕ
    // применяется vault-путём (его исполняет host/exec, 6c). Loud Failed > молчаливая псевдо-запись.
    if action.target.is_exec() {
        return DispatchOutcome::Failed(
            "exec-таргет не применяется vault-путём (host/exec — Фаза-3 6c)".into(),
        );
    }
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

/// Наблюдаемые ledger-текстовки исходов пропоуз-раунда — ЗАДАЮТСЯ ПО-ФИЧЕВО вызывателем
/// (предложение/навык/exec — РАЗНЫЕ наблюдаемые строки; их видят тесты/лог). Канон НЕ синтезирует их, а
/// берёт готовыми — дедуп конвейера (R-5c) НЕ должен молча унифицировать по-фичевые тексты.
struct LedgerCopy {
    /// `record_before`+dup-lookup не дали id — fail-closed резюме.
    record_fail: String,
    /// **KILL-SWITCH:** пауза ПОСЛЕ decide (re-check) — запись подавлена, строка остаётся proposed.
    paused: String,
    /// transition proposed→approved не применился (гонка/чужое состояние) — запись отменена.
    transition_fail: String,
    /// решение = Reject — исход в `ledger.finish(REJECTED)` И возвращаемый вызывателю.
    rejected: String,
}

/// Вердикт КАНОНА [`run_proposal_round`]: вызыватель маппит в СВОЙ финальный тип и делает apply-дельту
/// ТОЛЬКО на `Approved` (transition proposed→approved уже проведён каноном).
enum ProposalVerdict {
    /// transition proposed→approved УСПЕШЕН — вызыватель применяет (или возвращает Approved).
    Approved { action_id: i64 },
    /// НЕ применено ШТАТНО: решение Reject (finish сделан) ИЛИ пауза (kill-switch re-check) — исход-строка.
    Rejected(String),
    /// НЕ применено ПО СБОЮ: record_before/dup-lookup ИЛИ transition proposed→approved — fail-closed причина.
    Failed(String),
}

/// **КАНОН пропоуз-раунда (R-5c)** — единое ядро ТРЁХ бывших копий «propose→decide→transition»
/// ([`propose_and_decide`] vault, [`dispatch_skill_save`] навык, [`dispatch_exec_decision`] exec):
/// record_before proposed-строки + dup-lookup-фолбэк → `on_proposed(action_id)` (вызыватель эмитит СВОИ
/// события — `Proposal`+`Diff` | `ExecProposal` — и строит `ProposalBatch`; ЕДИНСТВЕННОЕ различие
/// поверхности) → `decide` → **KILL-SWITCH RE-CHECK: пауза (`agent_paused`) ПОСЛЕ decide и ПЕРЕД
/// transition — РОВНО ОДИН РАЗ здесь, на ВСЕХ трёх путях** (пауза между решением и применением ОБЯЗАНА
/// отменить применение; строка остаётся `proposed` → одобрить снова на un-pause; fail-closed) → transition
/// proposed→approved. Различия — ПАРАМЕТРАМИ (`entry`/`on_proposed`/`copy`); тексты — из `copy` (по-фичево);
/// apply-дельта — У ВЫЗЫВАТЕЛЯ (только на `Approved`).
async fn run_proposal_round(
    ledger: &AuditSink,
    entry: ActionEntry,
    propose_key: &str,
    agent_paused: &Arc<AtomicBool>,
    copy: LedgerCopy,
    decision_source: &Arc<dyn DecisionSource>,
    on_proposed: impl FnOnce(i64) -> ProposalBatch,
) -> ProposalVerdict {
    // (1) proposed-строка + dup-lookup-фолбэк (идемпотентность повтора; ключ "propose:…" строит вызыватель
    // в `entry`). Иная ошибка ledger ⇒ Failed (fail-closed).
    let action_id = match ledger.record_before(entry).await {
        Ok(id) => id,
        Err(_) => match audit::lookup_id(&ledger_reader(ledger), propose_key).await {
            Some(id) => id,
            None => return ProposalVerdict::Failed(copy.record_fail),
        },
    };

    // (2) вызыватель эмитит СВОИ события + строит батч (ПОСЛЕ proposed-строки, ДО решения; различие путей).
    let batch = on_proposed(action_id);

    // (3) спросить источник решений.
    match decision_source.decide(&batch).await.decision_for(action_id) {
        ItemDecision::Approve => {
            // (4) KILL-SWITCH (AGENT-5, чек-пойнт #3): re-check паузы ПОСЛЕ decide() и ПЕРЕД transition —
            // ЕДИНСТВЕННЫЙ на всех трёх путях. Источник мог думать долго / пауза взведена в это окно. Строку
            // оставляем `proposed` → одобрить снова на un-pause. Одобряющий DecisionSource не пробьёт паузу.
            if agent_paused.load(Ordering::Relaxed) {
                return ProposalVerdict::Rejected(copy.paused);
            }
            // (5) proposed→approved. Не применился (гонка / двойное решение / чужое состояние) ⇒ fail-closed.
            let promoted = audit::transition(
                &ledger_writer(ledger),
                propose_key,
                STATE_PROPOSED,
                STATE_APPROVED,
            )
            .await
            .unwrap_or(false);
            if promoted {
                ProposalVerdict::Approved { action_id }
            } else {
                ProposalVerdict::Failed(copy.transition_fail)
            }
        }
        // Reject ⇒ proposed→rejected (finish, терминал). Диск/exec НЕ трогаем.
        ItemDecision::Reject => {
            let _ = ledger
                .finish(propose_key, STATE_REJECTED, &copy.rejected, None)
                .await;
            ProposalVerdict::Rejected(copy.rejected)
        }
    }
}

/// Предложить (ledger `proposed` + эмиссия Proposal/Diff), спросить [`DecisionSource`] и применить
/// ТОЛЬКО при явном Approve (иначе Reject — диск не трогаем). Один айтем на вызов (батч = строки
/// `proposed` прогона; здесь — одно действие за диспетч, что и есть батч из одного айтема).
///
/// R-5c: тонкая обёртка над [`run_proposal_round`] — строит entry/copy + vault-события (Proposal+Diff)
/// и на [`ProposalVerdict::Approved`] делает apply-дельту [`apply_now`] (vault-rooted, с classify_hash).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn propose_and_decide(
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

    // entry proposed-строки (ключ "propose:…" отделён от apply-ключа — см. [`proposal_key`]).
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
        // ПРИВАТНОСТЬ (AGENT-6): долговечное резюме диффа — ТОЛЬКО редакция-гвард [`DiffSummary`]
        // (счётчики строк + статус-токен, БЕЗ сырого содержимого заметки).
        diff_summary: Some(DiffSummary::new(add, del, change_kind(action)).render()),
    };
    // Наблюдаемые ledger-текстовки — ПО-ФИЧЕВО «предложение …» (тесты/лог видят; НЕ унифицируем).
    let copy = LedgerCopy {
        record_fail: "ledger: не удалось записать строку предложения".to_string(),
        paused: format!(
            "предложение {rel}: агент на паузе (kill-switch) — запись подавлена (предложение \
             остаётся для повторного решения на un-pause)"
        ),
        transition_fail: format!(
            "предложение {rel}: одобрение не применено (строка не в состоянии proposed) — \
             запись отменена"
        ),
        rejected: format!("предложение {rel} отклонено — НЕ применено"),
    };
    // vault-дельта событий (Proposal+пер-файловый Diff) — различие поверхности (exec шлёт ExecProposal).
    let verdict = run_proposal_round(
        ledger,
        entry,
        &propose_key,
        agent_paused,
        copy,
        decision_source,
        |action_id| {
            events.emit(AgentEvent::Proposal {
                run_id,
                files: vec![ProposedFile {
                    path: rel.clone(),
                    add,
                    del,
                    status,
                    action_id,
                }],
            });
            events.emit(AgentEvent::Diff {
                path: rel.clone(),
                add,
                del,
                status,
            });
            ProposalBatch {
                run_id,
                items: vec![ProposalItem {
                    action_id,
                    target_rel: rel.clone(),
                    tier: tier.clone(),
                    add,
                    del,
                }],
            }
        },
    )
    .await;

    // Маппинг вердикта в DispatchOutcome + apply-дельта (vault-rooted, с classify_hash) ТОЛЬКО на Approved.
    match verdict {
        ProposalVerdict::Approved { action_id: _ } => Ok(apply_now(
            action,
            run_id,
            canon_root,
            ledger,
            classify_hash,
            agent_paused,
        )
        .await),
        ProposalVerdict::Rejected(s) => Ok(DispatchOutcome::Rejected(s)),
        ProposalVerdict::Failed(s) => Ok(DispatchOutcome::Failed(s)),
    }
}

/// SELF-LEARNING SL-7c: host-РЕШЕНИЕ + применение `SkillSave` под **skills_root** (НЕ vault-rooted
/// `dispatch_action`). R-5c: тонкая обёртка над [`run_proposal_round`] — classify (`SkillSave` НИКОГДА не
/// Auto: HardBlocked→Err / Confirm→propose) + pre-image из skills_root + entry/copy; apply-дельта на
/// Approved — [`apply_skill_save`] (skills_root-confined, обратимая). KILL-SWITCH re-check
/// (пауза ПОСЛЕ decide ПЕРЕД transition) — в каноне, единый для всех путей.
// Прод-путь (SL-7d): зовётся `SkillSaveCtx::apply` ← зарегистрированный `SkillSaveTool`.
#[allow(clippy::too_many_arguments)]
pub(in crate::actuator) async fn dispatch_skill_save(
    action: &Action,
    run_id: i64,
    policy: &DispatchPolicy,
    decision_source: &Arc<dyn DecisionSource>,
    events: &dyn EventSink,
    ledger: &AuditSink,
    skills_root: &Path,
) -> Result<(DispatchOutcome, bool), ToolError> {
    // Возврат: (DispatchOutcome, real_write). `real_write=true` ТОЛЬКО при `ApplyOutcome::Executed`
    // (реальная запись на диск) — НЕ при `AlreadyDone` (идемпотентный replay, диск не тронут). SL-7d
    // SkillSaveCtx бьёт `bump_save` ТОЛЬКО при real_write (save_count == число реальных записей; ревью
    // SL-7d: иначе in-run повтор байт-идентичного skill.save раздул бы save_count). `mark_agent_created`
    // идемпотентен и бьётся на любом Applied (но SkillSaveCtx гейтит его тем же флагом — provenance уже
    // записан первой записью).
    // Defense-in-depth: только SkillSave (вызывается из SkillSaveTool, SL-7d).
    if !matches!(action.target, ActionTarget::SkillSave { .. }) {
        return Err(ToolError::Exec(
            "dispatch_skill_save вызван не для SkillSave".into(),
        ));
    }
    let rel = action.target.rel().to_string();

    // (1) classify ПЕРВЫМ (чистый, без IO): skills-флаги из политики. SkillSave classify НИКОГДА Auto.
    let ctx = ClassifyCtx {
        root: skills_root,
        overwrite_threshold: policy.overwrite_threshold,
        shell_enable: policy.shell_enable,
        sandbox_available: policy.sandbox_available,
        learning_enabled: policy.learning_enabled,
        skills_root_configured: policy.skills_root_configured,
    };
    let tier = classify(action, &ctx);
    let reason = match &tier {
        // HardBlocked — ВСЕГДА Err (learning off / root не настроен / форма/vendor / путь). Диск не трогаем.
        RiskTier::HardBlocked(reason) => return Err(ToolError::Exec(block_message(reason))),
        // SkillSave недопустимо Auto (classify_skill_save это гарантирует). Defense-in-depth fail-closed.
        RiskTier::Auto => {
            return Ok((
                DispatchOutcome::Failed(
                    "SkillSave недопустимо Auto — внутренняя ошибка классификации".into(),
                ),
                false,
            ))
        }
        RiskTier::Confirm(r) => r.clone(),
    };
    let _ = &reason; // тир уже Confirm; держим для симметрии/будущего

    // (2) Pre-image из skills_root (для диффа + classify_hash) через КОНФАЙН-рубеж (как read_current_in_vault
    // для заметок): резолвим путь `confine_for_overwrite` (resolve+leaf-симлинк+хардлинк reject) ПЕРЕД
    // чтением — симлинк-escape наружу skills_root не утечёт в diff-счётчики (fail-closed; ревью SL-7c).
    // None ⇒ create (нет файла / отвергнут). apply_skill_save всё равно ре-конфайнит на записи.
    let current = {
        let root = skills_root.to_path_buf();
        let rel_p = std::path::PathBuf::from(&rel);
        tokio::task::spawn_blocking(move || {
            confine_for_overwrite(&root, &rel_p)
                .ok()
                .and_then(|abs| std::fs::read_to_string(abs).ok())
        })
        .await
        .ok()
        .flatten()
    };
    let current_ref = current.as_deref().unwrap_or("");
    let classify_hash = if current_ref.is_empty() {
        String::new()
    } else {
        crate::vault::content_hash(current_ref.as_bytes())
    };

    // (3) proposed-строка ledger + Proposal/Diff события (зеркало propose_and_decide).
    let proposed = proposed_content(action, current_ref);
    let (add, del) = line_diff(current_ref, &proposed);
    let status = file_status(action);
    let propose_key = proposal_key(run_id, action, &classify_hash);
    let entry = ActionEntry {
        run_id,
        idempotency_key: propose_key.clone(),
        tool_name: action.target.tool_name().to_string(),
        target_rel: Some(rel.clone()),
        risk_tier: tier.as_str().to_string(),
        state: STATE_PROPOSED.to_string(),
        content_hash: if current_ref.is_empty() {
            None
        } else {
            Some(classify_hash.clone())
        },
        diff_summary: Some(DiffSummary::new(add, del, change_kind(action)).render()),
    };
    // Наблюдаемые ledger-текстовки — ПО-ФИЧЕВО «навык …» (per-caller, НЕ унифицируем с vault/exec).
    let copy = LedgerCopy {
        record_fail: "ledger: не удалось записать строку предложения навыка".to_string(),
        paused: format!(
            "навык {rel}: агент на паузе (kill-switch) — запись подавлена (предложение остаётся)"
        ),
        transition_fail: format!(
            "навык {rel}: одобрение не применено (строка не в proposed) — запись отменена"
        ),
        rejected: format!("навык {rel}: предложение отклонено — НЕ сохранён"),
    };
    // Зеркало vault-дельты событий (Proposal+Diff); apply-дельта — apply_skill_save (см. ниже).
    let verdict = run_proposal_round(
        ledger,
        entry,
        &propose_key,
        &policy.agent_paused,
        copy,
        decision_source,
        |action_id| {
            events.emit(AgentEvent::Proposal {
                run_id,
                files: vec![ProposedFile {
                    path: rel.clone(),
                    add,
                    del,
                    status,
                    action_id,
                }],
            });
            events.emit(AgentEvent::Diff {
                path: rel.clone(),
                add,
                del,
                status,
            });
            ProposalBatch {
                run_id,
                items: vec![ProposalItem {
                    action_id,
                    target_rel: rel.clone(),
                    tier: tier.clone(),
                    add,
                    del,
                }],
            }
        },
    )
    .await;

    match verdict {
        // apply-делта (только на Approved): skills_root-confined обратимая запись. classify_hash →
        // drift-фенс в apply. real_write=true ТОЛЬКО для Executed (реальная запись); AlreadyDone —
        // идемпотентный replay (диск не тронут) → save_count НЕ бьём (ревью SL-7d).
        ProposalVerdict::Approved { action_id: _ } => {
            let ch = if classify_hash.is_empty() {
                None
            } else {
                Some(classify_hash.as_str())
            };
            Ok(
                match apply_skill_save(
                    action,
                    run_id,
                    skills_root,
                    ledger,
                    ch,
                    &policy.agent_paused,
                )
                .await
                {
                    ApplyOutcome::Executed { summary, .. } => {
                        (DispatchOutcome::Applied(summary), true)
                    }
                    ApplyOutcome::AlreadyDone(o) => (DispatchOutcome::Applied(o), false),
                    ApplyOutcome::PathEscape => (
                        DispatchOutcome::Failed(format!(
                            "навык {rel}: путь вне skills_root — запись отклонена"
                        )),
                        false,
                    ),
                    ApplyOutcome::Failed(e) => (DispatchOutcome::Failed(e), false),
                },
            )
        }
        ProposalVerdict::Rejected(s) => Ok((DispatchOutcome::Rejected(s), false)),
        ProposalVerdict::Failed(s) => Ok((DispatchOutcome::Failed(s), false)),
    }
}

/// Исход host-РЕШЕНИЯ по exec-таргету (Фаза-3, SANDBOX-6c). НЕ применяет — exec исполняется ВНУТРИ
/// песочницы (6c-2). На Approve несёт `ledger_action_id` (вызывающий минтит exec_token, привязанный к нему)
/// и `propose_key` (СТРОКА idempotency-ключа ledger-строки — redeem/finalize 6c-2c/2d фенсят переходы
/// approved→executing→executed|failed именно по ней; единый источник — этот `dispatch_exec_decision`,
/// не пересчитывать в exec_host во избежание дрейфа с записанной строкой).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecDecision {
    /// Одобрено: строка ledger проведена proposed→approved; `ledger_action_id` — её id, `propose_key` — её
    /// idempotency-ключ (для последующих ledger-переходов redeem/finalize).
    Approved {
        ledger_action_id: i64,
        propose_key: String,
    },
    /// Отклонено (PolicyDefault DENY / человек reject / гонка transition / пауза) — резюме.
    Rejected(String),
    /// Жёстко заблокировано (shell_enable=false / песочница недоступна) — фенсенная причина.
    HardBlocked(String),
}

/// РЕДАКЦИЯ-БЕЗОПАСНОЕ резюме exec-предложения для [`AgentEvent::ExecProposal`] (наблюдаемость, §5.6
/// приватность). Несёт ТОЛЬКО структурную форму: дотированное имя инструмента + (для git) bounded-токен
/// `op` + СЧЁТЧИК argv/args — НЕ сырые значения argv/env (там могли бы оказаться пути/секреты). Зеркало
/// дисциплины [`DiffSummary`]: событие к UI/логам по построению не несёт содержимого команды, только её
/// «силуэт». vault-таргеты сюда не приходят (exec-only путь) → fallback-метка `"exec"`.
pub(crate) fn exec_proposal_summary(action: &Action) -> String {
    match &action.target {
        ActionTarget::ShellRun { argv, .. } => {
            format!("shell.run · argv: {} токен(ов)", argv.len())
        }
        ActionTarget::ProcessSpawn { args, .. } => {
            format!("process.spawn · args: {} токен(ов)", args.len())
        }
        ActionTarget::GitOp { op, args } => {
            format!("git.op · {op} · args: {} токен(ов)", args.len())
        }
        ActionTarget::NoteCreate { .. }
        | ActionTarget::NoteEdit { .. }
        | ActionTarget::Frontmatter { .. }
        | ActionTarget::SkillSave { .. } => "exec".to_string(),
    }
}

/// Host-РЕШЕНИЕ по exec-таргету (SANDBOX-6c, спека §5.2 фаза `decide`): `classify_exec` (НИКОГДА Auto) →
/// под `Confirm` спрашивает [`DecisionSource`] (PolicyDefault=DENY headless; ChannelDecision=человек). На
/// Approve проводит ledger `proposed→approved` (write-before-act intent) и ВОЗВРАЩАЕТ `action_id` — НО НЕ
/// ИСПОЛНЯЕТ (исполнение ВНУТРИ песочницы, 6c-2). **Vault-apply-путь (`apply_action`/`apply_now`) НЕ
/// зовётся** — exec там fail-closed (РУБЕЖ-0). R-5c: тонкая обёртка над [`run_proposal_round`] с
/// exec-дельтой (без apply); `events` (6c-2g) эмитит [`AgentEvent::ExecProposal`] (редакция-безопасное
/// резюме) ПОСЛЕ записи proposed-строки и ДО решения; KILL-SWITCH re-check — в каноне.
pub async fn dispatch_exec_decision(
    action: &Action,
    run_id: i64,
    policy: &DispatchPolicy,
    decision_source: &Arc<dyn DecisionSource>,
    ledger: &AuditSink,
    canon_root: &Path,
    events: &dyn EventSink,
) -> ExecDecision {
    // ── РУБЕЖ 0 (зеркало apply.rs РУБЕЖ-0): exec-only fail-closed, АКТИВНО В RELEASE ──────────────
    // debug_assert компилируется прочь в release; sibling apply_now выбрал РАНТАЙМ-guard. Сейчас не-exec
    // сюда не дойдёт (единственный вызыватель — DispatchExecBackend, питаемый WireExecAction::try_into →
    // только exec), но если vault-таргет когда-либо просочится в release, Confirm-арм записал бы proposed-
    // строку с target_rel=None (теряя vault-путь) и заминтил бы exec_token на vault-правку. Отсекаем
    // структурно, не только в debug. debug_assert оставлен как ГРОМКАЯ документация инварианта в тестах.
    debug_assert!(
        action.target.is_exec(),
        "dispatch_exec_decision только для exec-таргетов"
    );
    if !action.target.is_exec() {
        return ExecDecision::Rejected(
            "не-exec таргет на exec-пути решения — отказано (fail-closed)".into(),
        );
    }
    let ctx = ClassifyCtx {
        root: canon_root,
        overwrite_threshold: policy.overwrite_threshold,
        shell_enable: policy.shell_enable,
        sandbox_available: policy.sandbox_available,
        learning_enabled: policy.learning_enabled,
        skills_root_configured: policy.skills_root_configured,
    };
    let tier = classify(action, &ctx);
    match &tier {
        RiskTier::HardBlocked(reason) => ExecDecision::HardBlocked(block_message(reason)),
        // exec НИКОГДА не Auto (classify_exec); fail-closed на случай регрессии classify.
        RiskTier::Auto => ExecDecision::Rejected(
            "exec-таргет неожиданно классифицирован Auto — отказано (fail-closed)".into(),
        ),
        RiskTier::Confirm(_) => {
            // classify_hash пуст (нет vault-контента); proposal_key стабилен по действию (exhaustive over exec).
            let propose_key = proposal_key(run_id, action, "");
            let entry = ActionEntry {
                run_id,
                idempotency_key: propose_key.clone(),
                tool_name: action.target.tool_name().to_string(),
                target_rel: None, // exec не имеет vault-цели
                risk_tier: tier.as_str().to_string(),
                state: STATE_PROPOSED.to_string(),
                content_hash: None,
                diff_summary: None, // exec — не дифф (ExecProposal-метрики — 6c-2)
            };
            // Наблюдаемые ledger-текстовки — ПО-ФИЧЕВО «exec …» (per-caller, НЕ унифицируем). exec НЕ
            // применяет vault-путём: на Approved канон лишь проводит proposed→approved, а мы возвращаем
            // action_id/propose_key (исполнение — ВНУТРИ песочницы, 6c-2). kill-switch re-check (пауза
            // ПОСЛЕ decide ПЕРЕД transition) — В КАНОНЕ, единый для всех трёх путей.
            let copy = LedgerCopy {
                record_fail: "ledger: не удалось записать строку exec-предложения".to_string(),
                paused: "exec-предложение: агент на паузе (kill-switch) — подавлено".to_string(),
                transition_fail: "exec-одобрение не применено (строка не в состоянии proposed)"
                    .to_string(),
                rejected: "exec-предложение отклонено".to_string(),
            };
            // ExecProposal (6c-2g): UI/лог видят намерение исполнить ПОСЛЕ записи proposed-строки и ДО
            // запроса решения. summary — редакция-безопасный силуэт ([`exec_proposal_summary`]); сырые
            // argv/значения/вывод сюда НЕ идут. exec-дельта событий (различие поверхности пути).
            let verdict = run_proposal_round(
                ledger,
                entry,
                &propose_key,
                &policy.agent_paused,
                copy,
                decision_source,
                |action_id| {
                    events.emit(AgentEvent::ExecProposal {
                        run_id,
                        action_id,
                        summary: exec_proposal_summary(action),
                    });
                    ProposalBatch {
                        run_id,
                        items: vec![ProposalItem {
                            action_id,
                            target_rel: String::new(),
                            tier: tier.clone(),
                            add: 0,
                            del: 0,
                        }],
                    }
                },
            )
            .await;
            match verdict {
                // exec НЕ применяет vault-путём — на Approved возвращаем id+ключ (исполнение в песочнице).
                ProposalVerdict::Approved { action_id } => ExecDecision::Approved {
                    ledger_action_id: action_id,
                    propose_key,
                },
                ProposalVerdict::Rejected(s) | ProposalVerdict::Failed(s) => {
                    ExecDecision::Rejected(s)
                }
            }
        }
    }
}

/// Ключ строки ПРЕДЛОЖЕНИЯ — отдельный от apply-ключа (префикс), чтобы не коллизировать с record_before
/// самого apply. Стабилен по `(run_id, tool, args, classify_hash)` — то же предложение даёт тот же ключ.
pub(crate) fn proposal_key(run_id: i64, action: &Action, classify_hash: &str) -> String {
    // EXHAUSTIVE (без `_ =>`): payload-репрезентация на каждый вариант. exec-таргеты не имеют content/value
    // → детерминированный payload из их полей (US-разделитель `\u{1f}`); tool_name() уже различает их.
    let payload: Option<String> = match &action.target {
        // SkillSave — content-несущая (тело SKILL.md), как create/edit: payload = content.
        ActionTarget::NoteCreate { .. }
        | ActionTarget::NoteEdit { .. }
        | ActionTarget::SkillSave { .. } => action.content.clone(),
        ActionTarget::Frontmatter { .. } => action.value.clone(),
        ActionTarget::ShellRun { argv, cwd_rel } => Some(format!(
            "{}\u{1f}{}",
            argv.join("\u{1f}"),
            cwd_rel.as_deref().unwrap_or("")
        )),
        ActionTarget::ProcessSpawn {
            program,
            args,
            cwd_rel,
        } => Some(format!(
            "{program}\u{1f}{}\u{1f}{}",
            args.join("\u{1f}"),
            cwd_rel.as_deref().unwrap_or("")
        )),
        ActionTarget::GitOp { op, args } => Some(format!("{op}\u{1f}{}", args.join("\u{1f}"))),
    };
    let args = canonical_args(Some(action.target.rel()), payload.as_deref());
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
