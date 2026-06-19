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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

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

/// Кумулятивный счётчик blast-radius прогона (число авто-применённых Auto-действий). Делим за
/// `Arc<AtomicU32>` между диспетчами одного прогона — анти-усталость: за кэпом Auto-тир форсирует
/// предложение вместо тихого авто-применения. Полный token-bucket/TTL — AGENT-5.
#[derive(Clone, Default)]
pub struct BlastRadius {
    applied: Arc<AtomicU32>,
}

impl BlastRadius {
    /// Новый счётчик (ноль авто-применений).
    pub fn new() -> Self {
        Self::default()
    }

    /// Текущее число авто-применений прогона.
    pub fn count(&self) -> u32 {
        self.applied.load(Ordering::SeqCst)
    }

    /// Инкремент после успешного авто-применения Auto-тира.
    fn bump(&self) {
        self.applied.fetch_add(1, Ordering::SeqCst);
    }

    /// Под кэпом ли ЕЩЁ ОДНО авто-применение (текущее число < cap).
    fn under_cap(&self, cap: u32) -> bool {
        self.count() < cap
    }
}

/// Политика автономии прогона + параметры гейта. `overwrite_threshold` — ИЗ КОНФИГА (run-policy), НЕ
/// хардкод-константа (3c hard-gate). `blast_radius_cap` — кэп кумулятивных авто-применений Auto-тира в
/// auto-прогоне (за ним — форс-предложение). `blast_radius` — общий на прогон счётчик.
#[derive(Clone)]
pub struct DispatchPolicy {
    /// `"auto"` ⇒ авто-применение Auto-тира; иначе (`"confirm"`/`None`/прочее) ⇒ предлагать всё.
    pub auto: bool,
    /// Порог «крупной перезаписи» (байт) → Confirm. Источник — конфиг прогона.
    pub overwrite_threshold: usize,
    /// Кэп авто-применений Auto-тира в auto-прогоне (анти-усталость). За ним Auto форсирует предложение.
    pub blast_radius_cap: u32,
    /// Кумулятивный счётчик авто-применений прогона (делится между диспетчами).
    pub blast_radius: BlastRadius,
}

impl DispatchPolicy {
    /// Собрать политику из автономии прогона (`Some("auto")` ⇒ auto, иначе confirm = безопаснее),
    /// порога перезаписи (конфиг) и кэпа blast-radius. `None` автономии ⇒ confirm (fail-safe).
    pub fn new(autonomy: Option<&str>, overwrite_threshold: usize, blast_radius_cap: u32) -> Self {
        Self {
            auto: matches!(autonomy, Some("auto")),
            overwrite_threshold,
            blast_radius_cap,
            blast_radius: BlastRadius::new(),
        }
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
            // auto-прогон + под blast-radius-кэпом ⇒ применить СРАЗУ (бамп счётчика).
            if policy.auto && policy.blast_radius.under_cap(policy.blast_radius_cap) {
                let out = apply_now(action, run_id, canon_root, ledger, &classify_hash).await;
                // Бампим blast-radius ТОЛЬКО на реально применённом (Applied) — Failed не «тратит» кэп.
                if matches!(out, DispatchOutcome::Applied(_)) {
                    policy.blast_radius.bump();
                }
                Ok(out)
            } else {
                // confirm-прогон (предлагать всё) ИЛИ auto за кэпом (анти-усталость) ⇒ предложить.
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
            )
            .await
        }
    }
}

/// Применить действие через [`apply_action`] с ОБЯЗАТЕЛЬНЫМ `classify_hash` (3c hard-gate) и свернуть
/// [`ApplyOutcome`] в [`DispatchOutcome`].
async fn apply_now(
    action: &Action,
    run_id: i64,
    canon_root: &Path,
    ledger: &AuditSink,
    classify_hash: &str,
) -> DispatchOutcome {
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
            Ok(apply_now(action, run_id, canon_root, ledger, classify_hash).await)
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
    use crate::actuator::audit::{lookup, STATE_EXECUTED};
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
    /// Кэп blast-radius теста.
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

    /// auto-run + Auto-тир ⇒ ПРИМЕНЯЕТ напрямую (НЕ предложение), blast-radius бампнут.
    #[tokio::test]
    async fn auto_run_auto_tier_applies_directly_bumps_blast() {
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
        assert_eq!(pol.blast_radius.count(), 1, "blast-radius бампнут");
        // НИ Proposal, НИ Diff (применено напрямую).
        assert!(
            !events
                .events()
                .iter()
                .any(|e| matches!(e, AgentEvent::Proposal { .. } | AgentEvent::Diff { .. })),
            "авто-применение НЕ эмитит предложение"
        );
    }

    /// auto-run + Auto-тир ЗА blast-radius-кэпом ⇒ ФОРСИРУЕТ предложение (анти-усталость).
    #[tokio::test]
    async fn auto_run_over_blast_cap_forces_proposal() {
        let (_d, root, sink) = setup().await;
        let events = CollectingSink::new();
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // reject all.
                                                                    // Кэп = 0 ⇒ даже первое Auto-действие за кэпом ⇒ предложение.
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
            "за кэпом — предложение"
        );
    }

    /// blast-radius ТОЧНАЯ граница (общий счётчик прогона): cap=2 ⇒ ПЕРВЫЕ ДВА Auto авто-применяются,
    /// ТРЕТЬЕ форсирует предложение (кумулятивно по диспетчам одной политики).
    #[tokio::test]
    async fn blast_radius_boundary_cap_then_propose() {
        let (_d, root, sink) = setup().await;
        let src: Arc<dyn DecisionSource> = Arc::new(PolicyDefault); // reject (для 3-го предложения).
        let pol = DispatchPolicy::new(Some("auto"), T, 2); // cap=2.

        // Действие 1 и 2 — Auto, под кэпом ⇒ применяются.
        for (i, rel) in ["B1.md", "B2.md"].iter().enumerate() {
            let events = CollectingSink::new();
            let action = Action::note_create(*rel, "x");
            let out = dispatch_action(&action, (i + 1) as i64, &pol, &src, &events, &sink, &root)
                .await
                .unwrap();
            assert!(matches!(out, DispatchOutcome::Applied(_)), "{rel}: {out:?}");
            assert!(root.join(rel).exists(), "{rel} записан");
        }
        assert_eq!(pol.blast_radius.count(), 2, "два авто-применения учтены");

        // Действие 3 — Auto, но ЗА кэпом ⇒ предложение (PolicyDefault reject ⇒ не записано).
        let events = CollectingSink::new();
        let action = Action::note_create("B3.md", "x");
        let out = dispatch_action(&action, 3, &pol, &src, &events, &sink, &root)
            .await
            .unwrap();
        assert!(matches!(out, DispatchOutcome::Rejected(_)), "3-е: {out:?}");
        assert!(
            !root.join("B3.md").exists(),
            "3-е НЕ записано (за кэпом → предложено)"
        );
        assert!(
            events
                .events()
                .iter()
                .any(|e| matches!(e, AgentEvent::Proposal { .. })),
            "3-е действие предложено"
        );
        assert_eq!(
            pol.blast_radius.count(),
            2,
            "предложение не бампит blast-radius"
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
        // Предложено (Confirm) и отклонено PolicyDefault ⇒ файл НЕ перезаписан, blast НЕ бампнут.
        assert!(matches!(out, DispatchOutcome::Rejected(_)), "out={out:?}");
        assert_eq!(read(&root, "E.md"), "orig", "Confirm в auto НЕ применился");
        assert_eq!(
            pol.blast_radius.count(),
            0,
            "Confirm не тратит blast-radius"
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

    // Доступ к reader sink'а для проверок ledger в тестах (зеркало apply.rs).
    fn sink_reader(sink: &AuditSink) -> crate::db::ReadPool {
        sink.reader_handle()
    }
}
