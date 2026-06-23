//! Файловые инструменты-актуаторы (AGENT-3c/3e, Фаза 1): `note.create` / `note.edit` /
//! `note.set_frontmatter` — ПЕРВЫЕ инструменты с побочным эффектом (запись в vault).
//!
//! Каждый реализует [`crate::agent::Tool`]. `invoke(args)`:
//!  1. строгий разбор аргументов (`serde` + `deny_unknown_fields`) → [`ToolError::BadArgs`] (I-4 fail-closed);
//!  2. сборка типизированного [`Action`];
//!  3. **маршрутизация ТОЛЬКО через ШОВ актуатора** [`ActionDispatcher`] — инструмент держит
//!     `Arc<dyn ActionDispatcher>` и НЕ знает транспорт. In-process реализация ([`GatedToolCtx`]) сводится
//!     к host-side [`orchestrate::dispatch_action`]; in-sandbox ([`crate::sandbox::act::ProxyActuator`]) —
//!     к `host/act` RPC, который на хосте применяет тем же `dispatch_action`. Гейт делает classify (порог
//!     из политики), матч `(RiskTier × autonomy)`, эмиссию Proposal/Diff, спрос [`DecisionSource`] и apply
//!     ТОЛЬКО одобренного с ОБЯЗАТЕЛЬНЫМ `classify_hash` — ВСЕГДА host-side (контейнер не решает);
//!  4. свёртка [`DispatchOutcome`] в строку-результат инструмента (Applied/Rejected → Ok; Failed →
//!     зафенсенная [`ToolError::Exec`], HardBlocked → [`ToolError::Exec`] изнутри гейта).
//!
//! ## AGENT-3e hard-gate #1 — НЕТ УНГЕЙТЕД-ПУТИ
//! 3c-helper `dispatch` (classify→ПРЯМОЙ `apply_action` для Auto, стаб-строка для Confirm) **УДАЛЁН**.
//! Инструмент БОЛЬШЕ НЕ зовёт [`apply_action`] напрямую и НЕ имеет ветки, минующей решение автономии:
//! ЕДИНСТВЕННЫЙ путь, которым зарегистрированный инструмент касается диска, — через [`ActionDispatcher`],
//! и ОБЕ его реализации сводятся к host-side [`orchestrate::dispatch_action`] (он один зовёт
//! `apply_action`). Шов транспорт-агностичен, но НЕ вводит обхода гейта: песочница лишь меняет МЕСТО
//! вызова инструмента (контейнер), authoritative-применение остаётся в ОДНОМ host-side `dispatch_action`.
//! Это акцептанс go-live: ни одно применение не происходит без гейта.
//!
//! ## Зависимости гейта несёт реализация шва ([`GatedToolCtx`])
//! In-process [`GatedToolCtx`] держит ВСЕ deps `dispatch_action`: `canon_root`, `ledger` ([`AuditSink`]),
//! `run_id`, [`DispatchPolicy`] (автономия прогона + `overwrite_threshold` из конфига + общий на прогон
//! токен-бакет [`super::orchestrate::TokenBucket`]), [`DecisionSource`] и [`EventSink`] — все за
//! [`Arc`] (дёшево клонируется, переживает прогон; политика делит токен-бакет авто-применений между
//! инструментами). In-sandbox реализация ([`ProxyActuator`]) этих deps НЕ держит — их держит host-side
//! бэкенд (`DispatchActuatorBackend`) за `host/act` RPC.
//!
//! ## Проводка (AGENT-3e)
//! Реестр гейтнутых инструментов СТРОИТСЯ ПО-ПРОГОННО в [`crate::agent::AgentRunHandler`] — и ТОЛЬКО
//! когда конфиг-флаг `agent_actuator_enabled` ВКЛ (по умолчанию ВЫКЛ → стабы, реальный vault не
//! затронут). Headless agentd собирает их с [`super::decision::PolicyDefault`] (auto-DENY).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::agent::{Tool, ToolError, ToolSpec};
use crate::db::WriteActor;

use super::action::Action;
use super::apply::AuditSink;
use super::decision::DecisionSource;
use super::orchestrate::{dispatch_action, dispatch_skill_save, DispatchPolicy, EventSink};

/// Порог «крупной перезаписи» (байт) по умолчанию для NoteEdit → Confirm(LargeOverwrite). РАЗУМНЫЙ
/// ДЕФОЛТ конфига (`ai.chat`/run-policy задаёт реальный порог). Гейт получает порог из
/// [`DispatchPolicy::overwrite_threshold`], а НЕ из этой константы — она лишь дефолт композиционного корня.
pub const OVERWRITE_THRESHOLD: usize = 64 * 1024;

/// Общий контекст гейтнутых файловых инструментов: ВСЕ зависимости [`dispatch_action`]. Держим за
/// [`Arc`] (инструменты дёшево клонируются в реестр и переживают прогон). [`DispatchPolicy`] несёт
/// общий на прогон [`super::orchestrate::TokenBucket`] — поэтому инструменты ДЕЛЯТ токен-бакет авто-
/// применений (анти-усталость работает кросс-инструментно в рамках одного прогона).
#[derive(Clone)]
pub struct GatedToolCtx {
    /// КАНОНИЗИРОВАННЫЙ корень vault (предусловие resolve_vault_path_for_write).
    pub canon_root: Arc<PathBuf>,
    /// Idempotency-ledger (`agent_actions`).
    pub ledger: Arc<AuditSink>,
    /// Идентификатор прогона (ledger-корреляция + idempotency_key).
    pub run_id: i64,
    /// Политика автономии прогона + порог перезаписи + blast-radius (общий счётчик).
    pub policy: DispatchPolicy,
    /// Источник решений по предложениям (headless agentd → [`super::decision::PolicyDefault`] auto-DENY).
    pub decision_source: Arc<dyn DecisionSource>,
    /// Приёмник Proposal/Diff (headless → [`super::orchestrate::TracingEventSink`]).
    pub events: Arc<dyn EventSink>,
}

impl GatedToolCtx {
    /// Собрать контекст из всех deps гейта. `canon_root`/`ledger` оборачиваются в [`Arc`] (политика и
    /// источники уже разделяемы).
    pub fn new(
        canon_root: PathBuf,
        ledger: AuditSink,
        run_id: i64,
        policy: DispatchPolicy,
        decision_source: Arc<dyn DecisionSource>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            canon_root: Arc::new(canon_root),
            ledger: Arc::new(ledger),
            run_id,
            policy,
            decision_source,
            events,
        }
    }
}

/// **ШОВ актуатора** — абстракция «применить действие через гейт автономии», ОТВЯЗЫВАЮЩАЯ файловые
/// инструменты от транспорта применения. Две реализации:
/// - [`GatedToolCtx`] (in-process) — зовёт host-side [`dispatch_action`] напрямую (классический путь);
/// - [`crate::sandbox::act::ProxyActuator`] (in-sandbox) — шлёт `host/act` RPC хосту (vault `:ro` в
///   контейнере → запись host-side), а хост применяет тем же `dispatch_action`.
///
/// Инструмент держит `Arc<dyn ActionDispatcher>` и НЕ знает, локально применяется действие или через RPC —
/// реестр инструментов ОДИН, транспорт actuator'а выбирает композиционный корень (`run_agent_session` →
/// `GatedToolCtx`; `--sandbox-child` → `ProxyActuator`). ЕДИНСТВЕННЫЙ путь применения (нет ungated-ветки,
/// 3e hard-gate #1): обе реализации сводятся к ОДНОМУ host-side `dispatch_action`.
#[async_trait]
pub trait ActionDispatcher: Send + Sync {
    /// Применить действие, свёрнутое в строку-результат инструмента: Applied/Rejected → `Ok(summary)`,
    /// Failed → зафенсенная [`ToolError::Exec`]; HardBlocked гейт сам отдаёт как `Err(Exec)`.
    async fn apply(&self, action: Action) -> Result<String, ToolError>;
}

#[async_trait]
impl ActionDispatcher for GatedToolCtx {
    /// In-process: ВСЕ deps `dispatch_action` несёт сам контекст (canon_root/ledger/run_id/policy/
    /// decision_source/events). НЕТ ветки, минующей решение автономии (3e hard-gate #1).
    async fn apply(&self, action: Action) -> Result<String, ToolError> {
        dispatch_action(
            &action,
            self.run_id,
            &self.policy,
            &self.decision_source,
            self.events.as_ref(),
            self.ledger.as_ref(),
            self.canon_root.as_path(),
        )
        .await?
        .into_tool_result()
    }
}

/// Аргументы [`NoteCreateTool`] / [`NoteEditTool`]: путь + тело. `deny_unknown_fields` (I-4).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathContentArgs {
    /// vault-rel путь заметки.
    path: String,
    /// Тело заметки.
    content: String,
}

/// Аргументы [`SetFrontmatterTool`]: путь + ключ + значение. `deny_unknown_fields` (I-4).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontmatterArgs {
    /// vault-rel путь заметки.
    path: String,
    /// Плоский top-level frontmatter-ключ.
    key: String,
    /// Скалярное значение ключа.
    value: String,
}

/// Строгий разбор сырых JSON-аргументов (пусто → `{}` → BadArgs о недостающих полях).
fn parse_args<T: for<'de> Deserialize<'de>>(args: &str) -> Result<T, ToolError> {
    let raw = if args.trim().is_empty() { "{}" } else { args };
    serde_json::from_str(raw).map_err(|e| ToolError::BadArgs(e.to_string()))
}

/// `note.create` — создаёт НОВУЮ заметку (fail-closed: цель не должна существовать).
pub struct NoteCreateTool {
    dispatcher: Arc<dyn ActionDispatcher>,
}

impl NoteCreateTool {
    pub fn new(dispatcher: Arc<dyn ActionDispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl Tool for NoteCreateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "note.create".into(),
            description:
                "Создаёт новую заметку по vault-rel пути с заданным телом. Цель не должна \
                          существовать (иначе ошибка). Только внутри vault."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "vault-rel путь новой заметки (напр. Notes/New.md)" },
                    "content": { "type": "string", "description": "Тело заметки" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: PathContentArgs = parse_args(args)?;
        self.dispatcher
            .apply(Action::note_create(a.path, a.content))
            .await
    }
}

/// `note.edit` — перезаписывает тело СУЩЕСТВУЮЩЕЙ заметки (снапшот-перед-правкой; крупная → Confirm).
pub struct NoteEditTool {
    dispatcher: Arc<dyn ActionDispatcher>,
}

impl NoteEditTool {
    pub fn new(dispatcher: Arc<dyn ActionDispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl Tool for NoteEditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "note.edit".into(),
            description:
                "Перезаписывает тело существующей заметки по vault-rel пути. Перед записью \
                          снимается снапшот истории (обратимость). Крупная перезапись требует \
                          подтверждения."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "vault-rel путь существующей заметки" },
                    "content": { "type": "string", "description": "Новое тело заметки" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: PathContentArgs = parse_args(args)?;
        self.dispatcher
            .apply(Action::note_edit(a.path, a.content))
            .await
    }
}

/// `note.set_frontmatter` — ставит ОДИН плоский top-level frontmatter-ключ (через единственный
/// санкционированный писатель `set_frontmatter_field`; снапшот-перед-правкой).
pub struct SetFrontmatterTool {
    dispatcher: Arc<dyn ActionDispatcher>,
}

impl SetFrontmatterTool {
    pub fn new(dispatcher: Arc<dyn ActionDispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl Tool for SetFrontmatterTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "note.set_frontmatter".into(),
            description:
                "Устанавливает один плоский top-level frontmatter-ключ существующей заметки \
                          (остальной YAML/тело сохраняются). Перед записью — снапшот истории."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "vault-rel путь существующей заметки" },
                    "key": { "type": "string", "description": "Плоский top-level frontmatter-ключ" },
                    "value": { "type": "string", "description": "Скалярное значение ключа" }
                },
                "required": ["path", "key", "value"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: FrontmatterArgs = parse_args(args)?;
        self.dispatcher
            .apply(Action::frontmatter(a.path, a.key, a.value))
            .await
    }
}

// ── SELF-LEARNING SL-7d: skill.save (авторство навыков агентом) ──────────────────────────────────

/// Имя первого сегмента rel `<name>/SKILL.md` (для провенанса usage). Пусто, если формат неожиданный
/// (defense; classify/tool это уже не пропустят).
fn skill_name_from_rel(rel: &str) -> String {
    rel.split('/').next().unwrap_or("").to_string()
}

/// **ШОВ навыка** (SL-7d, зеркало [`GatedToolCtx`], но через [`dispatch_skill_save`] под **skills_root**).
/// Несёт deps `dispatch_skill_save` + опц. писатель телеметрии для ПРОВЕНАНСА. На РЕАЛЬНОМ применении
/// (`DispatchOutcome::Applied`) проставляет `created_by='agent'` (`mark_agent_created` ПЕРВЫМ — INSERT-only)
/// и инкрементит `save_count` (`bump_save`) — порядок load-bearing (SL-1: mark до телеметрии, иначе навык
/// останется неуправляемым curator'ом). Rejected/Failed/HardBlocked провенанс НЕ пишут.
#[derive(Clone)]
pub struct SkillSaveCtx {
    /// КАНОНИЧЕСКИЙ корень skills (конфайн-база записи навыка; НЕ vault).
    pub skills_root: Arc<PathBuf>,
    pub ledger: Arc<AuditSink>,
    pub run_id: i64,
    /// Политика прогона (делит token-bucket/паузу с note-инструментами — общий blast-radius).
    pub policy: DispatchPolicy,
    pub decision_source: Arc<dyn DecisionSource>,
    pub events: Arc<dyn EventSink>,
    /// Писатель телеметрии навыков (провенанс). `None` → провенанс не пишется (навык не станет
    /// curator-управляемым; редкий путь без БД).
    pub recorder: Option<WriteActor>,
}

impl SkillSaveCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        skills_root: PathBuf,
        ledger: AuditSink,
        run_id: i64,
        policy: DispatchPolicy,
        decision_source: Arc<dyn DecisionSource>,
        events: Arc<dyn EventSink>,
        recorder: Option<WriteActor>,
    ) -> Self {
        Self {
            skills_root: Arc::new(skills_root),
            ledger: Arc::new(ledger),
            run_id,
            policy,
            decision_source,
            events,
            recorder,
        }
    }
}

#[async_trait]
impl ActionDispatcher for SkillSaveCtx {
    async fn apply(&self, action: Action) -> Result<String, ToolError> {
        let (outcome, real_write) = dispatch_skill_save(
            &action,
            self.run_id,
            &self.policy,
            &self.decision_source,
            self.events.as_ref(),
            self.ledger.as_ref(),
            self.skills_root.as_path(),
        )
        .await?;
        // ПРОВЕНАНС ТОЛЬКО при РЕАЛЬНОЙ записи (`real_write` = Executed, НЕ AlreadyDone-replay; ревью SL-7d:
        // иначе in-run повтор байт-идентичного skill.save раздул бы save_count). Порядок: mark_agent_created
        // (INSERT-only) → bump_save (SL-1 keystone). Ошибки телеметрии глотаем (не роняют результат).
        if real_write {
            if let Some(w) = &self.recorder {
                let name = skill_name_from_rel(action.target.rel());
                if !name.is_empty() {
                    let _ = crate::skills::usage::mark_agent_created(w, &name).await;
                    let _ = crate::skills::usage::bump_save(w, &name).await;
                }
            }
        }
        outcome.into_tool_result()
    }
}

/// Аргументы [`SkillSaveTool`]: имя навыка + однострочное описание + тело-инструкции. `deny_unknown_fields`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillSaveArgs {
    /// Имя навыка (станет каталогом `<name>/`); валидируется `skills::validate_name`.
    name: String,
    /// Однострочное назначение (frontmatter `description`).
    description: String,
    /// Тело-инструкции навыка (markdown после frontmatter).
    body: String,
}

/// Свернуть управляющие символы (вкл. `\n`/`\r`/`\t`) в пробел + обрезать края — `description` обязан
/// быть ОДНОЙ строкой (иначе многострочное значение «протекло» бы в тело frontmatter и сломало парс).
fn collapse_ws(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `skill.save` (SL-7d) — агент СОЗДАЁТ/перезаписывает СВОЙ навык `<name>/SKILL.md` под skills_root через
/// гейт (Confirm-НИКОГДА-Auto). Капабилити-потолок ПО ПОСТРОЕНИЮ: инструмент сам формирует frontmatter
/// (ТОЛЬКО `name`+`description`) — агент НЕ может объявить `capabilities`/`allowed-tools` (нет такого
/// аргумента; тело идёт ПОСЛЕ frontmatter, парсер берёт ПЕРВЫЙ блок). Регистрируется ТОЛЬКО при
/// `ai.skills.learning_enabled` + сконфигурированном skills_root + `agent_actuator_enabled` (default-OFF).
pub struct SkillSaveTool {
    dispatcher: Arc<dyn ActionDispatcher>,
}

impl SkillSaveTool {
    pub fn new(dispatcher: Arc<dyn ActionDispatcher>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl Tool for SkillSaveTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill.save".into(),
            description: "Сохраняет НАВЫК агента: создаёт/перезаписывает `<name>/SKILL.md` (инструкция, \
                          которую ты сможешь активировать позже). Имя — простой идентификатор (без `/`, \
                          `..`). Требует подтверждения (никогда не применяется автоматически)."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Имя навыка (каталог), напр. pdf-tools" },
                    "description": { "type": "string", "description": "Однострочное назначение навыка" },
                    "body": { "type": "string", "description": "Инструкции навыка (markdown)" }
                },
                "required": ["name", "description", "body"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let a: SkillSaveArgs = parse_args(args)?;
        // СИЛЬНАЯ валидация имени (как загрузчик): без `/`/`\`/`..`/control/пусто/огромное.
        crate::skills::validate_name(&a.name)
            .map_err(|e| ToolError::BadArgs(format!("недопустимое имя навыка: {e}")))?;
        // `vendor/` зарезервирован под hash-pinned вендоринг — агент туда не пишет (classify тоже режет).
        if a.name == crate::skills::VENDOR_DIR {
            return Err(ToolError::BadArgs(
                "имя `vendor` зарезервировано (вендоренные навыки неизменяемы)".into(),
            ));
        }
        let rel = format!("{}/SKILL.md", a.name);
        // frontmatter формирует ИНСТРУМЕНТ (только name+description) → агент не объявит capabilities.
        let content = format!(
            "---\nname: {}\ndescription: {}\n---\n{}",
            a.name,
            collapse_ws(&a.description),
            a.body
        );
        // Round-trip-фенс (defense; apply_skill_save проверит ещё раз): навык обязан перезагружаться.
        // Ловит непредставимое description (ведущая `[`/`{` после edge-stripper) — модель поправит.
        crate::skills::parse_skill(&content, &rel).map_err(|e| {
            ToolError::BadArgs(format!("SKILL.md невалиден ({e}) — поправь описание/тело"))
        })?;
        self.dispatcher
            .apply(Action::skill_save(rel, content))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::decision::{BatchDecision, ChannelDecision, ItemDecision, PolicyDefault};
    use crate::actuator::orchestrate::CollectingSink;
    use crate::agent::event::AgentEvent;
    use crate::db::Database;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Порог перезаписи теста (мал, чтобы крупная правка легко перешагнула в Confirm).
    const T: usize = 100;
    /// Кэп blast-radius теста.
    const CAP: u32 = 8;

    /// Временный vault + БД + AuditSink (canon_root канонизирован). Возвращаем dir, чтобы жил.
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

    /// Гейтнутый ctx с заданной автономией и источником решений (CollectingSink для событий).
    fn ctx_with(
        canon_root: &std::path::Path,
        sink: &AuditSink,
        autonomy: Option<&str>,
        decision_source: Arc<dyn DecisionSource>,
    ) -> GatedToolCtx {
        GatedToolCtx::new(
            canon_root.to_path_buf(),
            sink.clone(),
            1,
            DispatchPolicy::new(autonomy, T, CAP),
            decision_source,
            Arc::new(CollectingSink::new()),
        )
    }

    /// auto-прогон + PolicyDefault (не должен быть спрошен для Auto-тира), как `Arc<dyn ActionDispatcher>`
    /// (in-process ШОВ — `GatedToolCtx`), готовый к подаче в инструмент.
    fn auto_ctx(canon_root: &std::path::Path, sink: &AuditSink) -> Arc<dyn ActionDispatcher> {
        Arc::new(ctx_with(
            canon_root,
            sink,
            Some("auto"),
            Arc::new(PolicyDefault),
        ))
    }

    fn read(root: &std::path::Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    fn write_existing(root: &std::path::Path, rel: &str, content: &str) {
        let abs = root.join(rel);
        if let Some(p) = abs.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(abs, content).unwrap();
    }

    /// auto-run + note.create (Auto-тир) → ПРИМЕНЯЕТСЯ ЧЕРЕЗ ГЕЙТ (файл записан, резюме apply).
    #[tokio::test]
    async fn auto_run_note_create_applies_via_gate() {
        let (_d, root, sink) = setup().await;
        let t = NoteCreateTool::new(auto_ctx(&root, &sink));
        let res = t
            .invoke(r#"{"path":"Notes/N.md","content":"hi"}"#)
            .await
            .unwrap();
        assert!(res.contains("создана"), "резюме: {res}");
        assert_eq!(read(&root, "Notes/N.md"), "hi");
    }

    /// auto-run + note.edit малая (Auto) → перезапись через гейт; КРУПНАЯ (> порог) → Confirm-тир,
    /// который даже в auto-прогоне ПРЕДЛАГАЕТСЯ и под PolicyDefault auto-DENY-отклоняется (НЕ пишет).
    #[tokio::test]
    async fn auto_run_small_edit_applies_large_proposed_then_denied() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "E.md", "orig");
        let t = NoteEditTool::new(auto_ctx(&root, &sink));

        // Малая правка — Auto-тир, в auto-прогоне применяется через гейт.
        let res = t
            .invoke(r#"{"path":"E.md","content":"small edit"}"#)
            .await
            .unwrap();
        assert!(res.contains("отредактирована"), "резюме: {res}");
        assert_eq!(read(&root, "E.md"), "small edit");

        // Крупная правка (> T) — Confirm-тир: auto НЕ перекрывает Confirm → предложение → PolicyDefault
        // отклоняет → файл НЕ перезаписан. (Резюме гейта — «отклонено».)
        let big = "x".repeat(T + 1);
        let args = format!(r#"{{"path":"E.md","content":"{big}"}}"#);
        let res = t.invoke(&args).await.unwrap();
        assert!(
            res.contains("отклонено"),
            "Confirm под PolicyDefault: {res}"
        );
        assert_eq!(
            read(&root, "E.md"),
            "small edit",
            "Confirm-тир НЕ перезаписал файл (auto не override Confirm)"
        );
    }

    /// auto-run + note.set_frontmatter (Auto) → ставит ключ через гейт, сохраняет YAML/тело.
    #[tokio::test]
    async fn auto_run_set_frontmatter_applies_via_gate() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "F.md", "---\ntitle: T\n---\n\nbody\n");
        let t = SetFrontmatterTool::new(auto_ctx(&root, &sink));
        let res = t
            .invoke(r#"{"path":"F.md","key":"status","value":"done"}"#)
            .await
            .unwrap();
        assert!(res.contains("свойство"), "резюме: {res}");
        let new = read(&root, "F.md");
        assert!(new.contains("status: done") && new.contains("title: T"));
    }

    /// **3e hard-gate #1**: confirm-прогон + Auto-тир под PolicyDefault → инструмент ПРЕДЛАГАЕТ
    /// (Proposal/Diff эмитированы) и auto-DENY-отклоняется → файл НЕ записан. НЕТ ветки, которая бы
    /// применила инструмент в обход решения автономии (доказательство «нет ungated-пути»).
    #[tokio::test]
    async fn confirm_run_auto_tier_proposes_not_written_under_policy_default() {
        let (_d, root, sink) = setup().await;
        let events = Arc::new(CollectingSink::new());
        let ctx = GatedToolCtx::new(
            root.clone(),
            sink.clone(),
            1,
            DispatchPolicy::new(Some("confirm"), T, CAP),
            Arc::new(PolicyDefault),
            events.clone(),
        );
        let t = NoteCreateTool::new(Arc::new(ctx));

        let res = t
            .invoke(r#"{"path":"Notes/N.md","content":"hi"}"#)
            .await
            .unwrap();
        // Предложено и отклонено (PolicyDefault) — файл НЕ создан.
        assert!(res.contains("отклонено"), "résumé: {res}");
        assert!(
            !root.join("Notes/N.md").exists(),
            "confirm-run Auto под PolicyDefault: файл НЕ записан (нет ungated-пути)"
        );
        // Гейт реально ПРЕДЛОЖИЛ (Proposal эмитирован) — а не молча применил.
        assert!(
            events
                .events()
                .iter()
                .any(|e| matches!(e, AgentEvent::Proposal { .. })),
            "Auto-тир в confirm-прогоне эмитит Proposal"
        );
    }

    /// confirm-прогон + Auto-тир + Approve (ChannelDecision) → ПРИМЕНЯЕТСЯ через гейт (файл записан).
    /// Доказывает, что путь applied-через-гейт у инструмента работает (apply случается ТОЛЬКО по решению).
    #[tokio::test]
    async fn confirm_run_approve_applies_via_gate() {
        let (_d, root, sink) = setup().await;
        // action_id строки proposed в пустой БД = 1 (первый INSERT). Засеваем Approve по id=1.
        let (chan, tx) = ChannelDecision::new(1);
        tx.send(BatchDecision::from_pairs([(1, ItemDecision::Approve)]))
            .await
            .unwrap();
        let ctx = GatedToolCtx::new(
            root.clone(),
            sink.clone(),
            1,
            DispatchPolicy::new(Some("confirm"), T, CAP),
            Arc::new(chan),
            Arc::new(CollectingSink::new()),
        );
        let t = NoteCreateTool::new(Arc::new(ctx));

        let res = t
            .invoke(r#"{"path":"Notes/N.md","content":"hello"}"#)
            .await
            .unwrap();
        assert!(res.contains("создана"), "résumé: {res}");
        assert_eq!(read(&root, "Notes/N.md"), "hello", "записан после Approve");
    }

    /// HardBlocked (../escape, .nexus/x) → ToolError::Exec ИЗ ГЕЙТА (при любой автономии), диск НЕ
    /// тронут (файла нет). Апрув не разблокирует HardBlocked.
    #[tokio::test]
    async fn hardblocked_paths_error_no_write() {
        let (_d, root, sink) = setup().await;
        let create = NoteCreateTool::new(auto_ctx(&root, &sink));

        // Traversal-побег.
        let err = create
            .invoke(r#"{"path":"../escape.md","content":"x"}"#)
            .await;
        assert!(
            matches!(err, Err(ToolError::Exec(_))),
            "escape → Exec, было {err:?}"
        );
        assert!(
            !root.join("../escape.md").exists(),
            "файл вне vault не создан"
        );

        // Зарезервированный каталог.
        let err = create
            .invoke(r#"{"path":".nexus/secret.md","content":"x"}"#)
            .await;
        assert!(
            matches!(err, Err(ToolError::Exec(_))),
            "reserved → Exec, было {err:?}"
        );
        assert!(
            !root.join(".nexus/secret.md").exists(),
            "файл в .nexus не создан"
        );
    }

    /// Строгие аргументы: неизвестное поле / отсутствующее поле / не-JSON → BadArgs (I-4 fail-closed).
    /// Разбор аргументов происходит ДО гейта — ошибочный args не доходит до dispatch_action.
    #[tokio::test]
    async fn strict_args_bad_args() {
        let (_d, root, sink) = setup().await;
        let create = NoteCreateTool::new(auto_ctx(&root, &sink));
        let edit = NoteEditTool::new(auto_ctx(&root, &sink));
        let fm = SetFrontmatterTool::new(auto_ctx(&root, &sink));

        // Неизвестное поле (deny_unknown_fields).
        assert!(matches!(
            create
                .invoke(r#"{"path":"a.md","content":"x","extra":1}"#)
                .await,
            Err(ToolError::BadArgs(_))
        ));
        // Отсутствует обязательное поле.
        assert!(matches!(
            edit.invoke(r#"{"path":"a.md"}"#).await,
            Err(ToolError::BadArgs(_))
        ));
        // Пусто → {} → нет полей → BadArgs.
        assert!(matches!(fm.invoke("").await, Err(ToolError::BadArgs(_))));
        // Не-JSON.
        assert!(matches!(
            create.invoke("not json").await,
            Err(ToolError::BadArgs(_))
        ));
    }

    // ── SL-7d: skill.save (авторство навыков) ───────────────────────────────────────────────────
    const VALID_BODY: &str = "Делай то-то и то-то по шагам.";

    /// SkillSaveCtx с learning ON + ChannelDecision(approve) + recorder. skills_root — отдельный temp.
    /// Возвращает (tool, skills_root, reader) для проверки файла + провенанса.
    async fn skill_tool(
        autonomy: Option<&str>,
        learning: bool,
        decision: Arc<dyn DecisionSource>,
    ) -> (
        SkillSaveTool,
        TempDir,
        crate::db::ReadPool,
        crate::db::WriteActor,
    ) {
        let dir = TempDir::new().unwrap();
        let skills_root = dir.path().canonicalize().unwrap();
        let db = Database::open(skills_root.join(".nexus/nexus.db"))
            .await
            .unwrap();
        let sink = AuditSink::new(db.writer().clone(), db.reader().clone());
        let reader = db.reader().clone();
        let writer = db.writer().clone();
        std::mem::forget(db);
        let policy = DispatchPolicy::new(autonomy, T, CAP).with_skills_flags(learning, true);
        let ctx = SkillSaveCtx::new(
            skills_root.clone(),
            sink,
            1,
            policy,
            decision,
            Arc::new(CollectingSink::new()),
            Some(writer.clone()),
        );
        (SkillSaveTool::new(Arc::new(ctx)), dir, reader, writer)
    }

    /// learning ON + Approve → навык записан под skills_root + провенанс (created_by=agent, save_count=1).
    #[tokio::test]
    async fn skill_save_tool_approve_writes_and_provenance() {
        let (chan, tx) = ChannelDecision::new(1);
        tx.send(BatchDecision::from_pairs([(1, ItemDecision::Approve)]))
            .await
            .unwrap();
        let (t, dir, reader, _w) = skill_tool(Some("confirm"), true, Arc::new(chan)).await;
        let args =
            format!(r#"{{"name":"pdf-tools","description":"Работа с PDF","body":"{VALID_BODY}"}}"#);
        let res = t.invoke(&args).await.unwrap();
        assert!(res.contains("навык"), "резюме: {res}");
        let abs = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("pdf-tools/SKILL.md");
        assert!(abs.exists(), "SKILL.md записан");
        let content = std::fs::read_to_string(&abs).unwrap();
        assert!(content.contains("name: pdf-tools") && content.contains(VALID_BODY));
        // Провенанс: строка created_by='agent', save_count инкрементнут.
        let rec = crate::skills::usage::get_record(&reader, "pdf-tools")
            .await
            .unwrap()
            .expect("usage-строка создана провенансом");
        assert_eq!(rec.created_by.as_deref(), Some("agent"), "created_by=agent");
        assert_eq!(rec.save_count, 1, "save_count инкрементнут");
    }

    /// Регрессия (ревью SL-7d): повтор БАЙТ-идентичного skill.save в одном прогоне → 2-й = AlreadyDone
    /// (диск не тронут) → `save_count` НЕ инкрементится повторно (== число РЕАЛЬНЫХ записей = 1).
    #[tokio::test]
    async fn skill_save_tool_replay_does_not_double_count() {
        // ChannelDecision на 2 апрува: emit1 (file отсутствует → propose action_id=1), emit2 (file есть,
        // classify_hash иной → propose action_id=2). Оба одобряем; apply emit2 → AlreadyDone.
        let (chan, tx) = ChannelDecision::new(2);
        tx.send(BatchDecision::from_pairs([(1, ItemDecision::Approve)]))
            .await
            .unwrap();
        tx.send(BatchDecision::from_pairs([(2, ItemDecision::Approve)]))
            .await
            .unwrap();
        let (t, _dir, reader, _w) = skill_tool(Some("confirm"), true, Arc::new(chan)).await;
        let args = format!(r#"{{"name":"dup","description":"d","body":"{VALID_BODY}"}}"#);
        t.invoke(&args).await.unwrap(); // реальная запись (Executed) → save_count=1
        t.invoke(&args).await.unwrap(); // байт-идентично → AlreadyDone → НЕ бьём save_count

        let rec = crate::skills::usage::get_record(&reader, "dup")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rec.save_count, 1,
            "save_count == число РЕАЛЬНЫХ записей (replay не раздувает)"
        );
        assert_eq!(rec.created_by.as_deref(), Some("agent"));
    }

    /// learning OFF → classify HardBlocked(LearningDisabled) → Err, файл НЕ записан, провенанса нет.
    #[tokio::test]
    async fn skill_save_tool_learning_disabled_blocked() {
        let (t, dir, reader, _w) =
            skill_tool(Some("confirm"), false, Arc::new(PolicyDefault)).await;
        let args = format!(r#"{{"name":"x","description":"d","body":"{VALID_BODY}"}}"#);
        let res = t.invoke(&args).await;
        assert!(
            matches!(res, Err(ToolError::Exec(_))),
            "learning off → Err: {res:?}"
        );
        assert!(!dir.path().join("x").exists(), "навык НЕ записан");
        assert!(
            crate::skills::usage::get_record(&reader, "x")
                .await
                .unwrap()
                .is_none(),
            "провенанса нет (не Applied)"
        );
    }

    /// Reject (PolicyDefault) при learning ON → Rejected (Ok-резюме), файл НЕ записан, провенанса НЕТ
    /// (провенанс ТОЛЬКО на Applied).
    #[tokio::test]
    async fn skill_save_tool_reject_no_provenance() {
        let (t, dir, reader, _w) = skill_tool(Some("confirm"), true, Arc::new(PolicyDefault)).await;
        let args = format!(r#"{{"name":"y","description":"d","body":"{VALID_BODY}"}}"#);
        let res = t.invoke(&args).await.unwrap();
        assert!(res.contains("отклонено"), "Reject под PolicyDefault: {res}");
        assert!(
            !dir.path().join("y").exists(),
            "отклонённый навык не записан"
        );
        assert!(
            crate::skills::usage::get_record(&reader, "y")
                .await
                .unwrap()
                .is_none(),
            "Rejected → провенанс НЕ пишется"
        );
    }

    /// Невалидное имя (slash / vendor) → BadArgs ДО гейта (файл/строка не создаются).
    #[tokio::test]
    async fn skill_save_tool_bad_name_rejected() {
        let (t, _dir, _r, _w) = skill_tool(Some("confirm"), true, Arc::new(PolicyDefault)).await;
        for bad in ["a/b", "..", "vendor"] {
            let args = format!(r#"{{"name":"{bad}","description":"d","body":"{VALID_BODY}"}}"#);
            assert!(
                matches!(t.invoke(&args).await, Err(ToolError::BadArgs(_))),
                "имя {bad:?} → BadArgs"
            );
        }
    }

    /// Имена инструментов — дотированные kinds (идут в AgentEvent ToolCall.kind).
    #[tokio::test]
    async fn tool_names_are_dotted_kinds() {
        let (_d, root, sink) = setup().await;
        assert_eq!(
            NoteCreateTool::new(auto_ctx(&root, &sink)).spec().name,
            "note.create"
        );
        assert_eq!(
            NoteEditTool::new(auto_ctx(&root, &sink)).spec().name,
            "note.edit"
        );
        assert_eq!(
            SetFrontmatterTool::new(auto_ctx(&root, &sink)).spec().name,
            "note.set_frontmatter"
        );
    }
}
