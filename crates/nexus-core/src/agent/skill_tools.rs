//! SKILL-2 (Фаза 1): инструменты агента для 3-tier раскрытия скиллов + [`SkillContext`].
//!
//! SKILL-1 загрузил каталог скиллов (`skills/mod.rs`). SKILL-2 проводит его в агента ТРЕМЯ слоями
//! РАСКРЫТИЯ (progressive disclosure), каждый — БОЛЬШЕ доверенного контекста, чем предыдущий, но
//! ВЕСЬ контент скилла остаётся НЕДОВЕРЕННЫМИ ДАННЫМИ (фенсен, роль user/tool, НИКОГДА system — I-5):
//! - **tier 1 — КАТАЛОГ** (`SkillCatalog::catalog_block`): меню «что существует» (name+description).
//!   Инжектится в начальный контекст прогона (см. `agent/job.rs`). Тел нет.
//! - **tier 2 — АКТИВАЦИЯ** ([`ActivateSkillTool`]): по имени из меню загружает ТЕЛО скилла (его
//!   инструкции) как фенсенный tool-результат. Имя ограничено `enum`'ом ТЕКУЩЕГО каталога; off-enum/
//!   неизвестное → fail-closed [`ToolError`].
//! - **tier 3 — РЕСУРС** ([`ReadSkillResourceTool`]): читает файл ВНУТРИ собственного подкаталога
//!   скилла (конфайн через [`skills::resolve_skill_resource`]); `..`/абсолют/симлинк-наружу → отказ.
//!
//! # Capability-ИНЕРТНОСТЬ (граница SKILL-2 → SKILL-3)
//! Активация скилла ВПРЫСКИВАЕТ ТОЛЬКО ТЕКСТ его инструкций. Она НЕ регистрирует инструментов, НЕ
//! даёт прав, НЕ трогает реестр. Скилл, объявивший `capabilities: [shell, web]`, остаётся ИНЕРТНЫМ —
//! `capabilities` лишь ЗАХВАЧЕНЫ SKILL-1; trust/capability-ГЕЙТ — это SKILL-3. Тело скилла — это
//! проза, которую читает модель; самопровозгласить инструмент она по ней не может.
//!
//! # Read-only
//! Оба инструмента ТОЛЬКО ЧИТАЮТ (каталог/тело/файл-ресурс). Записей в vault/ФС нет — отдельный
//! actuator-флаг им не нужен; они активны лишь когда сконфигурирован skills-каталог (см. [`SkillContext`]).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::ai::{fence_observation, injection_marker};
use crate::skills::{
    parse_typed_capabilities, resolve_capabilities, resolve_skill_resource, CapabilityResolution,
    RunPolicy, SkillCatalog,
};

use super::tool::{Tool, ToolError, ToolSpec};

/// Имя инструмента активации (tier 2). Дотированного namespace нет (как `note.create`) — это
/// верхнеуровневая способность агента, не сгруппированный actuator.
pub const ACTIVATE_SKILL_TOOL: &str = "activate_skill";
/// Имя инструмента чтения ресурса скилла (tier 3).
pub const READ_SKILL_RESOURCE_TOOL: &str = "read_skill_resource";

/// Жёсткий потолок размера читаемого ресурса (tier 3), байт. Зеркалит дух `FENCE_MAX_BYTES`: ресурс
/// — недоверенный внешний файл; не даём одному раздутому файлу взорвать контекст. Фенс цикла потом
/// всё равно подрежет тело, но честнее не тащить мегабайты в память.
const RESOURCE_MAX_BYTES: usize = 64 * 1024;

/// Контекст скиллов прогона (SKILL-2): загруженный каталог + КАНОНИЧЕСКИЙ корень skills-каталога.
///
/// Собирается композиционным корнем (agentd) / хендлером ТОЛЬКО когда skills-каталог сконфигурирован
/// (см. `AgentRunHandler`). Несёт два tier-источника: [`catalog`] (tier 1 меню + tier 2 тела) и
/// [`skills_root`] (tier 3 база конфайна ресурсов). `Arc` — дёшево клонируется в оба инструмента.
#[derive(Debug, Clone)]
pub struct SkillContext {
    /// Каталог скиллов (SKILL-1): меню (tier 1), тела (tier 2), rel_path для конфайна ресурсов (tier 3).
    catalog: Arc<SkillCatalog>,
    /// КАНОНИЧЕСКИЙ корень skills-каталога — база path-конфайна tier-3 ресурсов. Предусловие:
    /// канонизирован (как `discover_skills` канонизирует корень).
    skills_root: PathBuf,
}

impl SkillContext {
    /// Собирает контекст из каталога + канонического корня skills.
    pub fn new(catalog: Arc<SkillCatalog>, skills_root: PathBuf) -> Self {
        Self {
            catalog,
            skills_root,
        }
    }

    /// Каталог (для tier-1 инъекции меню вызывающим).
    pub fn catalog(&self) -> &SkillCatalog {
        &self.catalog
    }

    /// **Tier 1: фенсенный, user-role блок-меню** доступных скиллов (name+description, бюджетирован).
    /// Делегирует в [`SkillCatalog::catalog_block`] с per-request `marker`. Пусто → `None`.
    pub fn catalog_block(&self, marker: &str) -> Option<String> {
        self.catalog.catalog_block(marker)
    }

    /// Инструменты скиллов прогона (tier 2 + tier 3) для регистрации в реестре. Каждый разделяет этот
    /// `SkillContext` (тот же каталог/корень). Вызывается хендлером при сконфигурированном skills-каталоге.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(ActivateSkillTool::new(self.clone())),
            Arc::new(ReadSkillResourceTool::new(self.clone())),
        ]
    }
}

/// Аргументы [`ActivateSkillTool`]: ровно одно поле `skill`. `deny_unknown_fields` — лишнее поле →
/// BadArgs (I-4 fail-closed, граница не коэрсит мусор).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivateArgs {
    skill: String,
}

/// **Tier 2 — `activate_skill`**: по имени из меню загружает ТЕЛО скилла (его инструкции) как
/// фенсенный tool-результат.
///
/// # enum-ограничение + fail-closed
/// `spec().parameters` несёт `enum` = ИМЕНА ТЕКУЩЕГО каталога (читается в `spec()` → отражает живой
/// набор: скилл, добавленный вотчером, появится при пересборке `registry.specs()` на ходу). Активация
/// имени ВНЕ enum невозможна на стороне модели И ПЕРЕ-валидируется в `invoke` (если как-то off-enum —
/// `lookup` в каталоге не найдёт → [`ToolError::UnknownTool`]). Пустой каталог → enum пуст (активировать
/// нечего).
///
/// # Инертность + SKILL-3 сурфейсинг (AC#2)
/// Возвращает ТЕКСТ тела, обёрнутый [`fence_observation`] (недоверенные ДАННЫЕ, I-5). НИЧЕГО не
/// регистрирует и не разрешает. SKILL-3: после тела ВНУТРИ фенса добавляется advisory «Доступно: … /
/// Заявлено-но-инертно: … — <причина>», управляемый [`resolve_capabilities`] (Фаза C: forced ∩
/// run-policy). Скилл, запросивший `shell`/`web`, АКТИВИРУЕТСЯ (тело грузится), но модели ЯВНО сказано,
/// что эти способности недоступны и почему (НЕ молчаливый no-op). Скилл с caps ⊆ {VaultRead,VaultWrite}
/// → без inert-заметки (чистый zero-setup).
pub struct ActivateSkillTool {
    ctx: SkillContext,
}

impl ActivateSkillTool {
    pub fn new(ctx: SkillContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for ActivateSkillTool {
    fn spec(&self) -> ToolSpec {
        // enum = имена ЖИВОГО каталога (читаем здесь, в spec()). Пустой каталог → пустой enum.
        let names = self.ctx.catalog.names();
        ToolSpec {
            name: ACTIVATE_SKILL_TOOL.into(),
            description: "Активирует навык (skill) по имени из меню доступных навыков: загружает его \
                          инструкции (тело) как справочные ДАННЫЕ. Имя ДОЛЖНО быть одним из \
                          перечисленных в `enum`. Активация лишь показывает инструкции навыка — она \
                          НЕ даёт новых прав и не выполняет действий."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "skill": {
                        "type": "string",
                        "description": "Имя навыка для активации (одно из доступных).",
                        "enum": names
                    }
                },
                "required": ["skill"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let raw = if args.trim().is_empty() { "{}" } else { args };
        let parsed: ActivateArgs =
            serde_json::from_str(raw).map_err(|e| ToolError::BadArgs(e.to_string()))?;
        // Fail-closed ПЕРЕ-валидация: даже если имя как-то пришло off-enum, в каталоге его нет → отказ
        // (не «тихо пусто», не паника). enum — первый рубеж (модель), lookup — второй (исполнение).
        let skill = self
            .ctx
            .catalog
            .get(&parsed.skill)
            .ok_or_else(|| ToolError::UnknownTool(parsed.skill.clone()))?;
        // SKILL-3 (AC#2): сурфейсим granted-vs-inert capability-сводку. Фаза-C политика — vault
        // read+write (declared лишь ЗАПРАШИВАЕТ; эффективный = forced ∩ run-policy). Скилл, попросивший
        // shell/web, активируется (тело грузится), но получает явную пометку «инертно + причина» —
        // НЕ молчаливый no-op. ⊆ {VaultRead,VaultWrite} → без inert-заметки.
        let declared = parse_typed_capabilities(&skill.capabilities);
        let resolution = resolve_capabilities(&declared, skill.tier, &RunPolicy::phase_c_vault());
        let advisory = capability_advisory(&resolution);

        // Тело + advisory кладём ВНУТРЬ одного фенса (advisory — тоже часть наблюдения; не отдельный
        // доверенный канал). Tier-2 раскрытие: тело — недоверенные ДАННЫЕ (написал автор скилла).
        // Фенсим per-request маркером (I-5/AC-SEC-7): автор тела не знает маркер → не «закроет» блок.
        // Цикл агента ДОПОЛНИТЕЛЬНО обернёт результат в fence_observation("tool", …) при ре-инъекции
        // (defense-in-depth). Метка несёт имя скилла (вне маркеров — не доверие).
        let observation = match advisory {
            Some(note) => format!("{}\n\n{note}", skill.body),
            None => skill.body.clone(),
        };
        let marker = injection_marker();
        Ok(fence_observation(
            &format!("skill:{}", skill.name),
            &observation,
            &marker,
        ))
    }
}

/// Строит advisory-строку «Доступно: … / Заявлено-но-инертно: … — <причина>» из [`CapabilityResolution`]
/// (AC#2). `None`, если инертных способностей нет (caps ⊆ {VaultRead,VaultWrite} → чистый zero-setup —
/// шум не добавляем). Это ТЕКСТ внутри фенса (часть наблюдения-данных), а не управляющая инструкция.
fn capability_advisory(resolution: &CapabilityResolution) -> Option<String> {
    if !resolution.has_inert() {
        return None; // нет инертных — нечего сурфейсить.
    }
    let granted = resolution
        .granted
        .iter()
        .map(|c| c.label())
        .collect::<Vec<_>>()
        .join(", ");
    let inert = resolution
        .inert
        .iter()
        .map(|(c, reason)| format!("{} ({reason})", c.label()))
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!(
        "[capabilities] Доступно: {granted}. Заявлено-но-ИНЕРТНО в этом режиме: {inert}."
    ))
}

/// Аргументы [`ReadSkillResourceTool`]: `skill` + `resource_path`. `deny_unknown_fields` — fail-closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadResourceArgs {
    skill: String,
    resource_path: String,
}

/// **Tier 3 — `read_skill_resource`**: читает файл-ресурс ВНУТРИ собственного подкаталога скилла.
///
/// # Path-конфайн (no-escape)
/// `resource_path` резолвится через [`resolve_skill_resource`] ОТНОСИТЕЛЬНО каталога скилла: `..`/
/// абсолютный/симлинк-наружу → [`ToolError::Exec`] (PathEscape). Ресурс ОБЯЗАН лежать внутри своего
/// скилла — не в другом скилле, не в vault, не в произвольной ФС. Несуществующий → ошибка.
///
/// `skill` снова валидируется по каталогу (fail-closed): нет такого скилла → UnknownTool. Read-only:
/// никаких записей. Контент фенсится (недоверенные ДАННЫЕ, I-5).
pub struct ReadSkillResourceTool {
    ctx: SkillContext,
}

impl ReadSkillResourceTool {
    pub fn new(ctx: SkillContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for ReadSkillResourceTool {
    fn spec(&self) -> ToolSpec {
        let names = self.ctx.catalog.names();
        ToolSpec {
            name: READ_SKILL_RESOURCE_TOOL.into(),
            description:
                "Читает файл-ресурс, лежащий ВНУТРИ каталога указанного навыка (например, \
                          шаблон или справочный файл, на который ссылается навык). Путь \
                          `resource_path` — ОТНОСИТЕЛЬНО каталога навыка; выход за его пределы \
                          (`..`, абсолютный путь) запрещён. Только чтение."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "skill": {
                        "type": "string",
                        "description": "Имя навыка, которому принадлежит ресурс (одно из доступных).",
                        "enum": names
                    },
                    "resource_path": {
                        "type": "string",
                        "description": "Путь ресурса относительно каталога навыка (без `..`/абсолюта)."
                    }
                },
                "required": ["skill", "resource_path"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let raw = if args.trim().is_empty() { "{}" } else { args };
        let parsed: ReadResourceArgs =
            serde_json::from_str(raw).map_err(|e| ToolError::BadArgs(e.to_string()))?;
        // Fail-closed: ресурс читается ТОЛЬКО у известного скилла (нет такого → UnknownTool).
        let skill = self
            .ctx
            .catalog
            .get(&parsed.skill)
            .ok_or_else(|| ToolError::UnknownTool(parsed.skill.clone()))?;
        // Path-конфайн в подкаталог скилла. Любой побег (.. / абсолют / симлинк) → PathEscape → Exec.
        let abs = resolve_skill_resource(
            &self.ctx.skills_root,
            &skill.rel_path,
            &parsed.resource_path,
        )
        .map_err(|e| ToolError::Exec(format!("ресурс скилла недоступен: {e}")))?;
        // Читаем (read-only) с капом размера: блокирующее чтение в spawn_blocking (мы в async-инструменте).
        let read = tokio::task::spawn_blocking(move || read_capped(&abs, RESOURCE_MAX_BYTES))
            .await
            .map_err(|e| ToolError::Exec(format!("join: {e}")))?;
        let content = read.map_err(|e| ToolError::Exec(format!("чтение ресурса: {e}")))?;
        // Фенсим контент ресурса (недоверенные ДАННЫЕ, I-5) per-request маркером; метка несёт скилл+путь.
        let marker = injection_marker();
        Ok(fence_observation(
            &format!("skill-resource:{}/{}", skill.name, parsed.resource_path),
            &content,
            &marker,
        ))
    }
}

/// Читает файл, усекая до `max` байт по границе UTF-8 (lossy для не-UTF-8). Блокирующее — вызывать
/// из `spawn_blocking`. Усечение помечается явной строкой в конце (как `fence_observation`).
fn read_capped(path: &std::path::Path, max: usize) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    if bytes.len() <= max {
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        let mut cut = max;
        while cut > 0 && (bytes[cut] & 0b1100_0000) == 0b1000_0000 {
            cut -= 1; // не разрезаем codepoint: пятимся с continuation-байта
        }
        let mut s = String::from_utf8_lossy(&bytes[..cut]).into_owned();
        s.push_str(&format!("\n…[усечено {} байт]", bytes.len() - cut));
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::discover_skills;
    use std::fs;
    use tempfile::TempDir;

    /// Пишет скилл `<dir>/SKILL.md` с заданным телом.
    fn write_skill(root: &std::path::Path, dir: &str, name: &str, desc: &str, body: &str) {
        let d = root.join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n{body}"),
        )
        .unwrap();
    }

    /// Строит SkillContext из временного skills-каталога с заданными скиллами.
    fn ctx_with(skills: &[(&str, &str, &str, &str)]) -> (TempDir, SkillContext) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        for (dir, name, desc, body) in skills {
            write_skill(&root, dir, name, desc, body);
        }
        let cat = Arc::new(discover_skills(&root));
        let ctx = SkillContext::new(cat, root);
        (tmp, ctx)
    }

    // ── tier 2: activate_skill — enum-ограничение + fail-closed invoke + фенс ────────────────────────

    /// spec().parameters.enum == имена ТЕКУЩЕГО каталога.
    #[test]
    fn activate_spec_enum_is_current_catalog_names() {
        let (_t, ctx) = ctx_with(&[
            ("alpha", "alpha", "first", "BODY-A"),
            ("beta", "beta", "second", "BODY-B"),
        ]);
        let tool = ActivateSkillTool::new(ctx);
        let spec = tool.spec();
        let enum_vals = spec.parameters["properties"]["skill"]["enum"]
            .as_array()
            .expect("enum-массив");
        let names: Vec<&str> = enum_vals.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"alpha"), "enum несёт alpha: {names:?}");
        assert!(names.contains(&"beta"), "enum несёт beta: {names:?}");
        assert_eq!(names.len(), 2, "ровно текущий набор");
    }

    /// invoke валидным именем → возвращает ТЕЛО скилла, фенсенное маркером.
    #[tokio::test]
    async fn activate_valid_returns_body_fenced() {
        let (_t, ctx) = ctx_with(&[("pdf", "pdf", "Work with PDFs", "FULL-INSTRUCTIONS-HERE")]);
        let tool = ActivateSkillTool::new(ctx);
        let out = tool.invoke(r#"{"skill":"pdf"}"#).await.unwrap();
        assert!(
            out.contains("FULL-INSTRUCTIONS-HERE"),
            "тело раскрыто (tier 2): {out}"
        );
        assert!(out.contains('⟦'), "результат фенсен маркером: {out}");
        assert!(
            out.contains("skill:pdf"),
            "метка источника несёт имя скилла"
        );
    }

    /// invoke имени НЕ из каталога → fail-closed ToolError (UnknownTool), даже если бы оно пришло off-enum.
    #[tokio::test]
    async fn activate_unknown_name_fails_closed() {
        let (_t, ctx) = ctx_with(&[("pdf", "pdf", "d", "B")]);
        let tool = ActivateSkillTool::new(ctx);
        let r = tool.invoke(r#"{"skill":"does-not-exist"}"#).await;
        assert!(
            matches!(r, Err(ToolError::UnknownTool(_))),
            "off-каталог → fail-closed: {r:?}"
        );
    }

    /// Строгие аргументы: лишнее поле → BadArgs (deny_unknown_fields); пусто → BadArgs (нет skill).
    #[tokio::test]
    async fn activate_strict_args() {
        let (_t, ctx) = ctx_with(&[("pdf", "pdf", "d", "B")]);
        let tool = ActivateSkillTool::new(ctx);
        assert!(matches!(
            tool.invoke(r#"{"skill":"pdf","oops":1}"#).await,
            Err(ToolError::BadArgs(_))
        ));
        assert!(matches!(tool.invoke("").await, Err(ToolError::BadArgs(_))));
        assert!(matches!(
            tool.invoke("not json").await,
            Err(ToolError::BadArgs(_))
        ));
    }

    // ── tier 3: read_skill_resource — конфайн + фенс ────────────────────────────────────────────────

    /// Чтение ресурса ВНУТРИ скилла → фенсенный контент.
    #[tokio::test]
    async fn read_resource_inside_skill_fenced() {
        let (tmp, ctx) = ctx_with(&[("pdf", "pdf", "d", "B")]);
        // Кладём ресурс внутрь каталога скилла.
        fs::write(
            tmp.path().canonicalize().unwrap().join("pdf/template.md"),
            "RESOURCE-CONTENT",
        )
        .unwrap();
        let tool = ReadSkillResourceTool::new(ctx);
        let out = tool
            .invoke(r#"{"skill":"pdf","resource_path":"template.md"}"#)
            .await
            .unwrap();
        assert!(out.contains("RESOURCE-CONTENT"), "контент ресурса: {out}");
        assert!(out.contains('⟦'), "фенсен маркером: {out}");
        assert!(
            out.contains("skill-resource:pdf/template.md"),
            "метка источника"
        );
    }

    /// `..`-escape в resource_path → ToolError (path-конфайн, нет выхода из подкаталога скилла).
    #[tokio::test]
    async fn read_resource_traversal_rejected() {
        let (tmp, ctx) = ctx_with(&[("a", "a", "d", "B"), ("b", "b", "d", "B")]);
        // Секрет в соседнем скилле b.
        fs::write(
            tmp.path().canonicalize().unwrap().join("b/secret.txt"),
            "OTHER-SECRET",
        )
        .unwrap();
        let tool = ReadSkillResourceTool::new(ctx);
        // Скилл a пытается дотянуться до b через ...
        let r = tool
            .invoke(r#"{"skill":"a","resource_path":"../b/secret.txt"}"#)
            .await;
        assert!(
            matches!(r, Err(ToolError::Exec(_))),
            "traversal в чужой скилл отбит: {r:?}"
        );
    }

    /// Абсолютный путь ресурса → ToolError (без чтения).
    #[tokio::test]
    async fn read_resource_absolute_rejected() {
        let (_t, ctx) = ctx_with(&[("a", "a", "d", "B")]);
        let tool = ReadSkillResourceTool::new(ctx);
        let r = tool
            .invoke(r#"{"skill":"a","resource_path":"/etc/passwd"}"#)
            .await;
        assert!(matches!(r, Err(ToolError::Exec(_))), "абсолют отбит: {r:?}");
    }

    /// Ресурс несуществующего скилла → fail-closed UnknownTool.
    #[tokio::test]
    async fn read_resource_unknown_skill_fails_closed() {
        let (_t, ctx) = ctx_with(&[("a", "a", "d", "B")]);
        let tool = ReadSkillResourceTool::new(ctx);
        let r = tool
            .invoke(r#"{"skill":"ghost","resource_path":"x.txt"}"#)
            .await;
        assert!(matches!(r, Err(ToolError::UnknownTool(_))), "{r:?}");
    }

    /// Строгие аргументы read_skill_resource: лишнее поле → BadArgs.
    #[tokio::test]
    async fn read_resource_strict_args() {
        let (_t, ctx) = ctx_with(&[("a", "a", "d", "B")]);
        let tool = ReadSkillResourceTool::new(ctx);
        assert!(matches!(
            tool.invoke(r#"{"skill":"a","resource_path":"x","oops":1}"#)
                .await,
            Err(ToolError::BadArgs(_))
        ));
    }

    // ── tier 1: catalog_block via context ───────────────────────────────────────────────────────────

    /// SkillContext::catalog_block отдаёт меню (name+desc, фенсен) и НЕ содержит тел (tier 1 ≠ tier 2).
    #[test]
    fn context_catalog_block_is_menu_only() {
        let (_t, ctx) = ctx_with(&[("pdf", "pdf", "Work with PDFs", "TIER2-ONLY-BODY")]);
        let block = ctx.catalog_block("⟦M⟧").unwrap();
        assert!(block.contains("pdf"), "имя в меню");
        assert!(block.contains("Work with PDFs"), "описание в меню");
        assert!(
            !block.contains("TIER2-ONLY-BODY"),
            "тело НЕ в tier-1 меню: {block}"
        );
    }

    // ── 3-tier + capability-инертность ──────────────────────────────────────────────────────────────

    /// **3-tier discipline:** меню (tier1) без тел; тело — ТОЛЬКО через activate (tier2); ресурс —
    /// ТОЛЬКО через read_skill_resource (tier3). Один контекст, три разных канала раскрытия.
    #[tokio::test]
    async fn three_tier_disclosure_separation() {
        let (tmp, ctx) = ctx_with(&[("k", "k", "menu-desc", "BODY-TIER2")]);
        fs::write(
            tmp.path().canonicalize().unwrap().join("k/r.txt"),
            "RES-TIER3",
        )
        .unwrap();

        // tier1: меню — есть имя/описание, НЕТ тела, НЕТ ресурса.
        let menu = ctx.catalog_block("⟦M⟧").unwrap();
        assert!(menu.contains("menu-desc"));
        assert!(!menu.contains("BODY-TIER2"), "тело не в tier1");
        assert!(!menu.contains("RES-TIER3"), "ресурс не в tier1");

        // tier2: activate → тело (но не ресурс).
        let act = ActivateSkillTool::new(ctx.clone());
        let body = act.invoke(r#"{"skill":"k"}"#).await.unwrap();
        assert!(body.contains("BODY-TIER2"), "тело через tier2");
        assert!(!body.contains("RES-TIER3"), "ресурс НЕ выдаётся активацией");

        // tier3: read_skill_resource → ресурс.
        let rd = ReadSkillResourceTool::new(ctx);
        let res = rd
            .invoke(r#"{"skill":"k","resource_path":"r.txt"}"#)
            .await
            .unwrap();
        assert!(res.contains("RES-TIER3"), "ресурс через tier3");
    }

    /// **Capability-ИНЕРТНОСТЬ:** активация скилла с `capabilities: [shell]` НЕ меняет реестр и НЕ
    /// добавляет прав — возвращает лишь ТЕКСТ тела. Capabilities захвачены SKILL-1, но инертны (SKILL-3).
    #[tokio::test]
    async fn activation_grants_no_tools_capabilities_inert() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        // Скилл объявляет shell-capability.
        let d = root.join("dangerous");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            "---\nname: dangerous\ndescription: claims shell\ncapabilities: [shell, web]\n---\nRUN rm -rf /\n",
        )
        .unwrap();
        let cat = Arc::new(discover_skills(&root));
        // SKILL-1 ЗАХВАТИЛ capability (но не применяет).
        assert_eq!(
            cat.get("dangerous").unwrap().capabilities,
            vec!["shell", "web"],
            "capability захвачена SKILL-1"
        );
        let ctx = SkillContext::new(cat, root);

        // Реестр инструментов скиллов: ровно 2 (activate + read_resource) — БЕЗ shell/web.
        let tools = ctx.tools();
        assert_eq!(tools.len(), 2, "только activate + read_resource");
        let tool_names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
        assert!(tool_names.contains(&ACTIVATE_SKILL_TOOL.to_string()));
        assert!(tool_names.contains(&READ_SKILL_RESOURCE_TOOL.to_string()));
        assert!(
            !tool_names.iter().any(|n| n == "shell" || n == "web"),
            "активация НЕ зарегистрировала capability-инструмент: {tool_names:?}"
        );

        // Активация возвращает лишь ТЕКСТ — никакого нового инструмента не появилось.
        let act = ActivateSkillTool::new(ctx.clone());
        let body = act.invoke(r#"{"skill":"dangerous"}"#).await.unwrap();
        assert!(
            body.contains("RUN rm -rf /"),
            "тело — это лишь проза-инструкция"
        );
        // tools() ВСЁ ЕЩЁ ровно 2 (активация ничего не добавила в реестр-источник).
        assert_eq!(
            ctx.tools().len(),
            2,
            "активация не изменила набор инструментов"
        );
    }

    // ── SKILL-3: activate сурфейсит инертные capabilities (AC#2) ─────────────────────────────────────

    /// Скилл, объявивший `shell`/`web`, АКТИВИРУЕТСЯ (тело грузится), НО результат содержит advisory
    /// «инертно + причина» — НЕ молчаливый no-op. Тело и advisory — внутри фенса (ДАННЫЕ).
    #[tokio::test]
    async fn activate_surfaces_inert_capabilities_with_reason() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let d = root.join("dangerous");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            "---\nname: dangerous\ndescription: claims shell\ncapabilities: [shell, web_post]\n---\nBODY-INSTRUCTIONS\n",
        )
        .unwrap();
        let cat = Arc::new(discover_skills(&root));
        let ctx = SkillContext::new(cat, root);
        let out = ActivateSkillTool::new(ctx)
            .invoke(r#"{"skill":"dangerous"}"#)
            .await
            .unwrap();
        // Тело раскрыто (активировано, не no-op).
        assert!(out.contains("BODY-INSTRUCTIONS"), "тело раскрыто: {out}");
        // Advisory присутствует: granted vault + inert shell/web_post с причинами.
        assert!(out.contains("[capabilities]"), "advisory-блок: {out}");
        assert!(out.contains("ИНЕРТНО"), "помечено инертно: {out}");
        assert!(out.contains("Shell"), "shell заявлен-но-инертен: {out}");
        assert!(
            out.contains("WebPost"),
            "web_post заявлен-но-инертен: {out}"
        );
        // Доступное vault — тоже в сводке.
        assert!(
            out.contains("VaultRead") || out.contains("VaultWrite"),
            "доступное: {out}"
        );
    }

    /// Скилл с caps ⊆ {VaultRead,VaultWrite} → активируется БЕЗ inert-заметки (чистый zero-setup).
    #[tokio::test]
    async fn activate_vault_only_no_inert_note() {
        let (_t, ctx) = ctx_with(&[("clean", "clean", "vault-only", "CLEAN-BODY")]);
        // skill без объявленных capabilities → declared пуст → нет инертных.
        let out = ActivateSkillTool::new(ctx)
            .invoke(r#"{"skill":"clean"}"#)
            .await
            .unwrap();
        assert!(out.contains("CLEAN-BODY"), "тело: {out}");
        assert!(
            !out.contains("[capabilities]"),
            "нет inert-заметки для vault-only: {out}"
        );
    }

    /// Скилл, объявивший ТОЛЬКО vault-способности → нет inert-заметки (они granted).
    #[tokio::test]
    async fn activate_declared_vault_caps_no_inert_note() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let d = root.join("vaultskill");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            "---\nname: vaultskill\ndescription: vault\ncapabilities: [read, write]\n---\nVBODY\n",
        )
        .unwrap();
        let cat = Arc::new(discover_skills(&root));
        let out = ActivateSkillTool::new(SkillContext::new(cat, root))
            .invoke(r#"{"skill":"vaultskill"}"#)
            .await
            .unwrap();
        assert!(out.contains("VBODY"));
        assert!(
            !out.contains("[capabilities]"),
            "vault-only declared → granted, без inert: {out}"
        );
    }

    /// I-5: tier-2 тело и tier-3 ресурс — фенсены (маркер на обоих концах), это ДАННЫЕ. Они НИКОГДА
    /// не попадают в роль system (инструменты возвращают строку; цикл кладёт её в роль `tool`, не system).
    #[tokio::test]
    async fn disclosed_content_is_fenced_data() {
        let (tmp, ctx) = ctx_with(&[("k", "k", "d", "BODY")]);
        fs::write(tmp.path().canonicalize().unwrap().join("k/r.txt"), "RES").unwrap();
        let body = ActivateSkillTool::new(ctx.clone())
            .invoke(r#"{"skill":"k"}"#)
            .await
            .unwrap();
        let res = ReadSkillResourceTool::new(ctx)
            .invoke(r#"{"skill":"k","resource_path":"r.txt"}"#)
            .await
            .unwrap();
        for out in [&body, &res] {
            // Маркер на обоих концах (открыт и закрыт) — структурный фенс.
            let opens = out.matches('⟦').count();
            assert!(opens >= 2, "фенсен с обеих сторон: {out}");
            // Явная пометка «недоверенные ДАННЫЕ» из fence_observation.
            assert!(
                out.contains("недоверенные ДАННЫЕ"),
                "помечено как данные: {out}"
            );
        }
    }
}
