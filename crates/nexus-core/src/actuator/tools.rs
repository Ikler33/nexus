//! Файловые инструменты-актуаторы (AGENT-3c, Фаза 1): `note.create` / `note.edit` /
//! `note.set_frontmatter` — ПЕРВЫЕ инструменты с побочным эффектом (запись в vault).
//!
//! Каждый реализует [`crate::agent::Tool`]. `invoke(args)`:
//!  1. строгий разбор аргументов (`serde` + `deny_unknown_fields`) → [`ToolError::BadArgs`] (I-4 fail-closed);
//!  2. сборка типизированного [`Action`];
//!  3. [`classify`] (ctx: canon_root + overwrite_threshold);
//!  4. диспетч по тиру риска:
//!     - HardBlocked(reason) → [`ToolError::Exec`] (зафенсенная ошибка — цикл выживает, модель восстановится);
//!     - Auto → [`apply_action`] → строка-резюме (tool-результат);
//!     - Confirm(reason) → **3c: «proposed — awaiting approval (not applied)», БЕЗ записи** (см. ниже).
//!
//! ## Граница 3c / 3d (Confirm-seam)
//! Здесь Confirm НЕ исполняется и НЕ пишет — возвращается явная строка «предложено, ожидает подтверждения
//! (не применено)». Настоящее предложение (эмиссия Proposal/Diff AgentEvent), DecisionSource и enforcement
//! автономии (confirm|auto на уровне прогона) — AGENT-3d. `overwrite_threshold` здесь — РАЗУМНАЯ КОНСТАНТА
//! [`OVERWRITE_THRESHOLD`]; в 3d/agentd порог придёт из конфигурации прогона. Это намеренный шов: см. TODO.
//!
//! ## Граница 3c / 3e (нет проводки)
//! Инструменты ЗДЕСЬ только конструируются и гоняются В ТЕСТАХ. Регистрации в `ToolRegistry`/agentd и
//! живой проводки НЕТ — это AGENT-3e (после autonomy-гейта 3d). Поэтому реальный vault пользователя
//! этим срезом не затрагивается: все дисковые записи — во временных vault'ах тестов.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::agent::{Tool, ToolError, ToolSpec};

use super::action::Action;
use super::apply::{apply_action, ApplyOutcome, AuditSink};
use super::classify::{classify, BlockReason, ClassifyCtx, RiskTier};

/// Порог «крупной перезаписи» (байт) для NoteEdit → Confirm(LargeOverwrite). РАЗУМНАЯ КОНСТАНТА 3c.
/// TODO(AGENT-3d): источник — конфигурация прогона (run-policy), не хардкод; ctx будет собираться из неё.
pub const OVERWRITE_THRESHOLD: usize = 64 * 1024;

/// Общий контекст файловых инструментов: канонизированный корень vault + ledger + run_id прогона.
/// Держим за [`Arc`] — инструменты дёшево клонируются в реестр и переживают прогон.
#[derive(Clone)]
pub struct FileToolCtx {
    /// КАНОНИЗИРОВАННЫЙ корень vault (предусловие resolve_vault_path_for_write).
    pub canon_root: Arc<PathBuf>,
    /// Idempotency-ledger (`agent_actions`).
    pub ledger: Arc<AuditSink>,
    /// Идентификатор прогона (ledger-корреляция + idempotency_key).
    pub run_id: i64,
}

impl FileToolCtx {
    /// Собрать контекст из канон-корня, ledger и run_id.
    pub fn new(canon_root: PathBuf, ledger: AuditSink, run_id: i64) -> Self {
        Self {
            canon_root: Arc::new(canon_root),
            ledger: Arc::new(ledger),
            run_id,
        }
    }

    fn classify_ctx(&self) -> ClassifyCtx<'_> {
        ClassifyCtx {
            root: self.canon_root.as_path(),
            overwrite_threshold: OVERWRITE_THRESHOLD,
        }
    }
}

/// Сообщение HardBlocked для зафенсенной ошибки инструмента (модель видит причину и переспрашивает).
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

/// Сообщение «предложено, ожидает подтверждения (не применено)» для Confirm-тира (3c-seam).
fn proposed_message(rel: &str) -> String {
    format!(
        "предложено — ожидает подтверждения (НЕ применено): перезапись {rel} превышает порог \
         авто-применения. Подтверждение/применение — AGENT-3d."
    )
}

/// Общий диспетч classify→apply/propose для всех трёх инструментов. Возвращает строку-результат
/// (Auto/Confirm) либо [`ToolError`] (HardBlocked/ошибка apply). Confirm и HardBlocked НЕ пишут на диск.
async fn dispatch(ctx: &FileToolCtx, action: Action) -> Result<String, ToolError> {
    let rel = action.target.rel().to_string();
    match classify(&action, &ctx.classify_ctx()) {
        // HardBlocked — зафенсенная ошибка (цикл выживает). Диск НЕ трогаем.
        RiskTier::HardBlocked(reason) => Err(ToolError::Exec(block_message(&reason))),
        // Confirm — 3c: proposed-not-applied. БЕЗ записи. (Реальный аппрув — 3d.)
        RiskTier::Confirm(_) => Ok(proposed_message(&rel)),
        // Auto — исполняем через apply_action (все рубежи внутри).
        RiskTier::Auto => {
            // classify_hash=None: 3c-инструмент не несёт at-classify on-disk hash (его источник —
            // changeset/proposal 3d). apply делает существенные рубежи (canonicalize/existence/ledger/
            // snapshot) и при None пропускает ТОЛЬКО drift-сравнение — остальные рубежи в силе.
            match apply_action(
                &action,
                ctx.run_id,
                ctx.canon_root.as_path(),
                &ctx.ledger,
                None,
            )
            .await
            {
                ApplyOutcome::Executed { summary, .. } => Ok(summary),
                ApplyOutcome::AlreadyDone(outcome) => {
                    Ok(format!("уже применено ранее (идемпотентно): {outcome}"))
                }
                // PathEscape ловит симлинк ВНУТРИ vault наружу (lexical classify пропустил как Auto) —
                // зафенсенная ошибка, диск (внешняя цель) НЕ тронут.
                ApplyOutcome::PathEscape => Err(ToolError::Exec(format!(
                    "путь {rel} разрешился ВНЕ vault (симлинк-побег) — запись заблокирована"
                ))),
                ApplyOutcome::Failed(reason) => Err(ToolError::Exec(reason)),
            }
        }
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
    ctx: FileToolCtx,
}

impl NoteCreateTool {
    pub fn new(ctx: FileToolCtx) -> Self {
        Self { ctx }
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
        dispatch(&self.ctx, Action::note_create(a.path, a.content)).await
    }
}

/// `note.edit` — перезаписывает тело СУЩЕСТВУЮЩЕЙ заметки (снапшот-перед-правкой; крупная → Confirm).
pub struct NoteEditTool {
    ctx: FileToolCtx,
}

impl NoteEditTool {
    pub fn new(ctx: FileToolCtx) -> Self {
        Self { ctx }
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
        dispatch(&self.ctx, Action::note_edit(a.path, a.content)).await
    }
}

/// `note.set_frontmatter` — ставит ОДИН плоский top-level frontmatter-ключ (через единственный
/// санкционированный писатель `set_frontmatter_field`; снапшот-перед-правкой).
pub struct SetFrontmatterTool {
    ctx: FileToolCtx,
}

impl SetFrontmatterTool {
    pub fn new(ctx: FileToolCtx) -> Self {
        Self { ctx }
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
        dispatch(&self.ctx, Action::frontmatter(a.path, a.key, a.value)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Временный vault + БД + FileToolCtx (canon_root канонизирован). Возвращаем dir, чтобы жил.
    async fn setup() -> (TempDir, PathBuf, FileToolCtx) {
        let dir = TempDir::new().unwrap();
        let canon_root = dir.path().canonicalize().unwrap();
        let db = Database::open(canon_root.join(".nexus/nexus.db"))
            .await
            .unwrap();
        let sink = AuditSink::new(db.writer().clone(), db.reader().clone());
        std::mem::forget(db); // writer/reader клонированы в sink — актор жив, пока жив клон.
        let ctx = FileToolCtx::new(canon_root.clone(), sink, 1);
        (dir, canon_root, ctx)
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

    /// note.create: пишет новую заметку (Auto), возвращает резюме.
    #[tokio::test]
    async fn note_create_writes() {
        let (_d, root, ctx) = setup().await;
        let t = NoteCreateTool::new(ctx);
        let res = t
            .invoke(r#"{"path":"Notes/N.md","content":"hi"}"#)
            .await
            .unwrap();
        assert!(res.contains("создана"), "резюме: {res}");
        assert_eq!(read(&root, "Notes/N.md"), "hi");
    }

    /// note.edit: малая правка (Auto) → перезапись; крупная → Confirm proposed-not-applied (НЕ пишет).
    #[tokio::test]
    async fn note_edit_small_auto_large_proposed() {
        let (_d, root, ctx) = setup().await;
        write_existing(&root, "E.md", "orig");
        let t = NoteEditTool::new(ctx);

        // Малая правка — Auto, пишет.
        let res = t
            .invoke(r#"{"path":"E.md","content":"small edit"}"#)
            .await
            .unwrap();
        assert!(res.contains("отредактирована"));
        assert_eq!(read(&root, "E.md"), "small edit");

        // Крупная правка (> OVERWRITE_THRESHOLD) — Confirm → proposed, НЕ применяется.
        let big = "x".repeat(OVERWRITE_THRESHOLD + 1);
        let args = format!(r#"{{"path":"E.md","content":"{big}"}}"#);
        let res = t.invoke(&args).await.unwrap();
        assert!(
            res.contains("ожидает подтверждения") && res.contains("НЕ применено"),
            "Confirm-резюме: {res}"
        );
        assert_eq!(
            read(&root, "E.md"),
            "small edit",
            "Confirm НЕ перезаписал файл"
        );
    }

    /// note.set_frontmatter: ставит ключ (Auto), сохраняет YAML/тело.
    #[tokio::test]
    async fn set_frontmatter_writes_key() {
        let (_d, root, ctx) = setup().await;
        write_existing(&root, "F.md", "---\ntitle: T\n---\n\nbody\n");
        let t = SetFrontmatterTool::new(ctx);
        let res = t
            .invoke(r#"{"path":"F.md","key":"status","value":"done"}"#)
            .await
            .unwrap();
        assert!(res.contains("свойство"), "резюме: {res}");
        let new = read(&root, "F.md");
        assert!(new.contains("status: done") && new.contains("title: T"));
    }

    /// HardBlocked (../escape, .nexus/x) → ToolError::Exec, диск НЕ тронут (файла нет).
    #[tokio::test]
    async fn hardblocked_paths_error_no_write() {
        let (_d, root, ctx) = setup().await;
        let create = NoteCreateTool::new(ctx.clone());

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
    #[tokio::test]
    async fn strict_args_bad_args() {
        let (_d, _root, ctx) = setup().await;
        let create = NoteCreateTool::new(ctx.clone());
        let edit = NoteEditTool::new(ctx.clone());
        let fm = SetFrontmatterTool::new(ctx);

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

    /// Имена инструментов — дотированные kinds (идут в AgentEvent ToolCall.kind).
    #[tokio::test]
    async fn tool_names_are_dotted_kinds() {
        let (_d, _root, ctx) = setup().await;
        assert_eq!(NoteCreateTool::new(ctx.clone()).spec().name, "note.create");
        assert_eq!(NoteEditTool::new(ctx.clone()).spec().name, "note.edit");
        assert_eq!(
            SetFrontmatterTool::new(ctx).spec().name,
            "note.set_frontmatter"
        );
    }
}
