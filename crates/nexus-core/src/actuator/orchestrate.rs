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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::event::FileStatus;
use crate::tool_types::ToolError;

// R-5b: самодостаточные единицы вынесены в подмодули (чистый перенос кода). Публичные имена
// реэкспортируются здесь без изменения внешних путей (`orchestrate::TokenBucket`/`EventSink`/…),
// поэтому `super::mod`-реэкспорты и тесты (`use super::*`) видят их как раньше.
mod events;
mod token_bucket;

pub use events::{CollectingSink, EventSink, TracingEventSink};
#[cfg(any(test, feature = "test-util"))]
pub use token_bucket::ManualClock;
pub use token_bucket::{
    Clock, MonotonicClock, TokenBucket, DEFAULT_REFILL_PER, DEFAULT_REFILL_TOKENS,
};

use super::action::{Action, ActionTarget};
use super::apply::AuditSink;
use super::audit::{ChangeKind, DiffSummary};
use super::classify::{classify, BlockReason, ClassifyCtx, RiskTier};
use super::decision::DecisionSource;

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
    /// **Фаза-3 (6b):** `ai.shell_enable` прогона → питает `ClassifyCtx` exec-таргетов. DEFAULT false
    /// (fail-safe; exec → HardBlocked(ShellDisabled)). Ставится через [`DispatchPolicy::with_exec_flags`].
    pub shell_enable: bool,
    /// **Фаза-3 (6b):** доступна ли песочница (`sandbox_enabled` И Linux) — ПРЕДвычислено корнем. DEFAULT
    /// false (fail-safe; exec → HardBlocked(SandboxUnavailable)).
    pub sandbox_available: bool,
    /// **SL-7:** `ai.skills.learning_enabled` прогона → питает `ClassifyCtx` для `SkillSave`. DEFAULT
    /// false (fail-safe; SkillSave → HardBlocked(LearningDisabled)). Ставится [`with_skills_flags`].
    pub learning_enabled: bool,
    /// **SL-7:** сконфигурирован ли skills_root (`ai.agent_skills_dir` задан) — ПРЕДвычислено корнем.
    /// DEFAULT false (fail-safe; SkillSave → HardBlocked(SkillsRootUnconfigured)).
    pub skills_root_configured: bool,
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
            shell_enable: false,
            sandbox_available: false,
            learning_enabled: false,
            skills_root_configured: false,
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
            shell_enable: false,
            sandbox_available: false,
            learning_enabled: false,
            skills_root_configured: false,
        }
    }

    /// **Фаза-3 (6b):** проставить exec-флаги (`shell_enable` из конфига + `sandbox_available` =
    /// `sandbox_enabled` И Linux, предвычислено корнем). Builder-стиль; DEFAULT обоих — false (fail-safe).
    pub fn with_exec_flags(mut self, shell_enable: bool, sandbox_available: bool) -> Self {
        self.shell_enable = shell_enable;
        self.sandbox_available = sandbox_available;
        self
    }

    /// **SL-7:** проставить skills-флаги (`learning_enabled` из конфига `ai.skills.learning_enabled` +
    /// `skills_root_configured` = `ai.agent_skills_dir` задан, предвычислено корнем). Builder-стиль;
    /// DEFAULT обоих — false (fail-safe; SkillSave → HardBlocked).
    pub fn with_skills_flags(
        mut self,
        learning_enabled: bool,
        skills_root_configured: bool,
    ) -> Self {
        self.learning_enabled = learning_enabled;
        self.skills_root_configured = skills_root_configured;
        self
    }

    /// Взведён ли kill-switch (пауза агента) — fail-safe: `true` ⇒ актуатор НЕ должен писать.
    /// `pub(crate)`: exec-redeem ([`crate::sandbox::exec_host`], 6c-2c) re-check'ит паузу ПЕРЕД проводкой
    /// approved-токена в EXECUTING (единый источник семантики паузы, не дубль чтения Arc).
    pub(crate) fn is_paused(&self) -> bool {
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
        BlockReason::ShellDisabled => {
            "host-исполнение выключено (ai.shell_enable=false) — действие заблокировано".to_string()
        }
        BlockReason::SandboxUnavailable => {
            "песочница недоступна (не-Linux / sandbox_enabled=false) — host-исполнение заблокировано"
                .to_string()
        }
        BlockReason::LearningDisabled => {
            "самообучение выключено (ai.skills.learning_enabled=false) — сохранение навыка заблокировано"
                .to_string()
        }
        BlockReason::SkillsRootUnconfigured => {
            "каталог навыков не настроен (ai.agent_skills_dir не задан) — некуда сохранять навык"
                .to_string()
        }
        BlockReason::InvalidSkillTarget => {
            "цель навыка должна быть `<имя>/SKILL.md` вне служебного `vendor/` — сохранение заблокировано"
                .to_string()
        }
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
        // SkillSave — content-несущая запись (тело SKILL.md): proposed = его content (для diff +N/-M
        // current→proposed, переиспользует apply_skill_save в SL-7c).
        ActionTarget::NoteCreate { .. }
        | ActionTarget::NoteEdit { .. }
        | ActionTarget::SkillSave { .. } => action.content.clone().unwrap_or_default(),
        ActionTarget::Frontmatter { key, .. } => {
            let value = action.value.clone().unwrap_or_default();
            crate::parser::set_frontmatter_field(current, key, &value)
                .unwrap_or_else(|_| current.to_string())
        }
        // Фаза-3 exec-таргеты — НЕ vault-запись, диффа нет (по этому пути не идут: classify→HardBlocked
        // по умолчанию, Confirm-propose — 6c). Инертно "" (вызовётся лишь если 6c протащит exec в propose).
        ActionTarget::ShellRun { .. }
        | ActionTarget::ProcessSpawn { .. }
        | ActionTarget::GitOp { .. } => String::new(),
    }
}

/// `FileStatus` по виду действия: create → New, edit/frontmatter → Edit.
fn file_status(action: &Action) -> FileStatus {
    match &action.target {
        ActionTarget::NoteCreate { .. } => FileStatus::New,
        ActionTarget::NoteEdit { .. } | ActionTarget::Frontmatter { .. } => FileStatus::Edit,
        // SL-7: SkillSave не идёт vault-changeset-путём (свой dispatch_skill_save); инертно New здесь.
        ActionTarget::SkillSave { .. } => FileStatus::New,
        // exec не порождает changeset-файл в 6b; инертно Edit (6c решит отдельный статус, если понадобится).
        ActionTarget::ShellRun { .. }
        | ActionTarget::ProcessSpawn { .. }
        | ActionTarget::GitOp { .. } => FileStatus::Edit,
    }
}

/// [`ChangeKind`] по виду действия (для долговечного `diff_summary` журнала): create → New,
/// edit/frontmatter → Edit, exec → Exec. Зеркало [`file_status`], но в audit-типе (журнал не тащит
/// UI-FileStatus). Exec-ветка классифицируется как [`ChangeKind::Exec`] для КОРРЕКТНОСТИ, но по
/// exec-пути `diff_summary` в журнал НЕ пишется (`dispatch_exec_decision` ставит `None`) — exec вне
/// vault-diff (нет `+N -M`).
fn change_kind(action: &Action) -> ChangeKind {
    match &action.target {
        ActionTarget::NoteCreate { .. } => ChangeKind::New,
        ActionTarget::NoteEdit { .. } | ActionTarget::Frontmatter { .. } => ChangeKind::Edit,
        // SL-7: запись навыка — свой токен (не vault new/edit); create-vs-overwrite несут +N/-M.
        ActionTarget::SkillSave { .. } => ChangeKind::SkillSave,
        ActionTarget::ShellRun { .. }
        | ActionTarget::ProcessSpawn { .. }
        | ActionTarget::GitOp { .. } => ChangeKind::Exec,
    }
}

/// ЕДИНЫЙ источник долговечного `diff_summary` (AGENT-6, приватность) — собирает [`DiffSummary`] из
/// СЧЁТЧИКОВ строк диффа `current → proposed` + [`ChangeKind`]. Переиспользуется и пропоуз-путём
/// ([`propose_and_decide`]), и авто-apply-путём ([`super::apply::apply_action`]) — оба пишут ИМЕННО эту
/// редакция-гард-форму, поэтому ни один писатель колонки не может занести в журнал сырой текст.
/// Счётчики получает [`line_diff`] (как в 3d-Diff); content тут НЕ хранится — только его счётчики.
pub(in crate::actuator) fn diff_summary_for(action: &Action, current: &str) -> DiffSummary {
    let proposed = proposed_content(action, current);
    let (add, del) = line_diff(current, &proposed);
    DiffSummary::new(add, del, change_kind(action))
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
    let is_exec = action.target.is_exec();

    // (1) Текущее содержимое цели IN-VAULT → classify_hash (токен на момент classify) + база диффа.
    // None (нет файла / побег) ⇒ classify_hash="" (конвенция apply Рубежа 3: on_disk_hash.unwrap_or("")).
    // exec-таргеты НЕ vault-цели (rel=="") → НЕ читаем диск (нечего; classify их не зависит от content).
    // SL-7 SkillSave: `rel` — skills_root-rel, НЕ vault → читать его в canon_root БЕССМЫСЛЕННО и неверно
    // (база рассинхрона). Сегодня недостижимо (classify→HardBlocked, нет инструмента), но defense-in-depth:
    // НЕ трогаем vault для SkillSave (его pre-image возьмёт apply_skill_save из skills_root, SL-7c).
    let skip_vault_read = is_exec || matches!(action.target, ActionTarget::SkillSave { .. });
    let current = if skip_vault_read {
        None
    } else {
        read_current_in_vault(canon_root, &rel).await
    };
    let classify_hash: String = current
        .as_deref()
        .map(|c| crate::vault::content_hash(c.as_bytes()))
        .unwrap_or_default();

    // (2) classify с порогом ИЗ КОНФИГА (не 64KiB-константа) + Фаза-3 exec-флаги из политики.
    let ctx = ClassifyCtx {
        root: canon_root,
        overwrite_threshold: policy.overwrite_threshold,
        shell_enable: policy.shell_enable,
        sandbox_available: policy.sandbox_available,
        learning_enabled: policy.learning_enabled,
        skills_root_configured: policy.skills_root_configured,
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

// R-5c: конвейер propose→decide→transition (`apply_now` + канон `run_proposal_round` + три обёртки +
// `dispatch_exec_decision`) вынесен в подмодуль `propose` (byte-identical; закрывает цель <1000 для
// `orchestrate.rs`). Публичные имена реэкспортируются здесь без изменения внешних путей
// (`orchestrate::dispatch_exec_decision`/…); общие хелперы гейта подмодуль берёт из `super::`.
mod propose;

pub(in crate::actuator) use propose::dispatch_skill_save;
pub use propose::{dispatch_exec_decision, ExecDecision};
// `dispatch_action` (ниже) зовёт оба.
pub(crate) use propose::{apply_now, propose_and_decide};
// Только для оракула `orchestrate/tests.rs` (через `use super::*`) — в прод-сборке не зовутся из `orchestrate`.
#[cfg(test)]
pub(crate) use propose::{exec_proposal_summary, proposal_key};

// Имена, которые прод-код гейта больше НЕ зовёт (конвейер уехал в `propose`), но которые оракул
// `orchestrate/tests.rs` видит через `use super::*` — реимпортируем ТОЛЬКО под тестами (иначе unused).
#[cfg(test)]
use super::audit::{STATE_APPROVED, STATE_REJECTED};
#[cfg(test)]
use super::decision::{ItemDecision, ProposalBatch};
#[cfg(test)]
use crate::event::AgentEvent;

#[cfg(test)]
mod tests;
