//! PURE fail-closed классификатор риска действия (AGENT-3b) — keystone безопасности (ADR-009 D4).
//!
//! [`classify`] — ЧИСТАЯ детерминированная функция `(&Action, &ClassifyCtx) -> RiskTier`: один и тот же
//! вход всегда даёт один и тот же тир, БЕЗ файлового IO. Это намеренно: классификатор — точка, где
//! решается «можно ли это сделать автоматически». Если бы он трогал ФС (canonicalize), он стал бы
//! недетерминированным (зависел бы от состояния диска/симлинков в момент вызова) и трудно-тестируемым.
//! Вместо этого он делает ЛЕКСИЧЕСКУЮ проверку конфайнмента пути ([`path_confinement`]) — строго
//! fail-closed (см. ниже). Канонизирующая (анти-симлинк/TOCTOU) проверка
//! [`crate::vault::resolve_vault_path_for_write`] применяется ДОПОЛНИТЕЛЬНО в `apply` (AGENT-3c) у самой
//! записи — два рубежа, не один. Лексическая проверка здесь — НАДмножество запретов: всё, что отвергает
//! canonicalize-граница на побег, отвергает и она (абсолют/root/`..`), плюс она режет ВСЕ dot-компоненты
//! как зарезервированные. Поэтому понизить риск она не может — только повысить блокировку.
//!
//! ## EXHAUSTIVE match, NO catch-all (инвариант D4)
//! [`classify`] матчит [`ActionTarget`] БЕЗ `_ =>`-ветки: каждый вариант решён ЯВНО. Это keystone
//! «no catch-all-downgrade»: если в [`crate::actuator::action`] добавят новый вид действия (например,
//! shell под Фазу-3), компилятор ЗАСТАВИТ дописать ветку в classify — новый вид НЕ провалится молча в
//! «Auto» через catch-all. Тест [`tests::adding_variant_breaks_match`] документирует это намерение.

use std::path::{Component, Path};

use super::action::{Action, ActionTarget};
use crate::skills::{SKILL_FILE, VENDOR_DIR};

/// Причина жёсткой блокировки (HardBlocked) — действие НЕ исполняется ни авто, ни по подтверждению.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// Путь выходит за пределы vault (абсолютный/root-anchored или `..`-traversal).
    PathEscape,
    /// Путь ведёт в зарезервированный/служебный каталог (`.nexus`/`.git` и прочие dot-компоненты).
    ReservedPath,
    /// Пустой/невалидный rel-путь (нет имени файла, пустая строка).
    EmptyPath,
    /// **SANDBOX-6a (Фаза-3):** host exec-таргет (`ShellRun`/`ProcessSpawn`/`GitOp`, вводятся в 6b) при
    /// ВЫКЛЮЧЕННОМ `ai.shell_enable` → жёсткий отказ (фича есть, но не разрешена владельцем).
    ShellDisabled,
    /// **SANDBOX-6a (Фаза-3):** host exec-таргет, когда песочница недоступна СТРУКТУРНО — не-Linux ИЛИ
    /// `ai.sandbox_enabled=false`. Block by-construction (§9): без OS-границы произвольное исполнение НЕ
    /// допускается даже при `shell_enable` (исполнять было бы негде безопасно).
    SandboxUnavailable,
    /// **SELF-LEARNING SL-7:** `SkillSave` при ВЫКЛЮЧЕННОМ `ai.skills.learning_enabled` → жёсткий отказ
    /// (фича есть, но самообучение не разрешено владельцем). Дефолт-OFF гейт авто-авторства навыков.
    LearningDisabled,
    /// **SELF-LEARNING SL-7:** `SkillSave`, когда skills_root НЕ сконфигурирован (`ai.agent_skills_dir`
    /// не задан) → некуда писать. Block by-construction: без явного корня навыков НЕ падаем молча в
    /// vault-root (это был бы PathEscape/перезапись заметок).
    SkillsRootUnconfigured,
    /// **SELF-LEARNING SL-7:** `SkillSave` с целью НЕ формы `<имя>/SKILL.md` ИЛИ в зарезервированном
    /// `vendor/`-неймспейсе. Keystone-инвариант (ревью SL-7a+b, MAJOR): агент-авторство пишет РОВНО
    /// `<name>/SKILL.md` (один сегмент-имя + файл) и НИКОГДА не в `vendor/` (hash-pinned вендоренные
    /// навыки неизменяемы — DB-провенанс их защищает в lifecycle, но НЕ от on-disk клоббера; режем
    /// ЛЕКСИЧЕСКИ здесь, в keystone, как заметки режут `.nexus`/`.git`). Защищает и от записи чужих
    /// ресурсов (`other/references/x`) и коллизии имён.
    InvalidSkillTarget,
}

/// Причина, по которой действие требует ПОДТВЕРЖДЕНИЯ пользователя (Confirm) перед исполнением.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmReason {
    /// Перезапись существующей заметки телом крупнее порога (`overwrite_threshold` байт): большой
    /// блок-радиус потери данных → не авто, спросить.
    LargeOverwrite,
    /// **Фаза-3 (SANDBOX-6b):** host exec-таргет при ВКЛЮЧЁННОМ `shell_enable` + доступной песочнице.
    /// Произвольное исполнение НИКОГДА не Auto — ВСЕГДА требует явного апрува (исполнится in-sandbox, 6c).
    ExecRequiresApproval,
    /// **SELF-LEARNING SL-7:** `SkillSave` при включённом learning + сконфигурированном skills_root.
    /// Авторство/перезапись навыка (будущих инструкций агента) НИКОГДА не Auto — ВСЕГДА явный апрув.
    SkillSaveRequiresApproval,
}

/// Тир риска — решение классификатора. ПОРЯДОК серьёзности: `Auto < Confirm < HardBlocked`.
/// `HardBlocked` — терминальный отказ (никакой апрув не разблокирует); `Confirm` — нужен апрув юзера;
/// `Auto` — можно исполнить без подтверждения (под автономией auto-режима).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskTier {
    /// Безопасно исполнить автоматически.
    Auto,
    /// Требует явного подтверждения пользователя.
    Confirm(ConfirmReason),
    /// Заблокировано наглухо — не исполнять никогда (даже с апрувом).
    HardBlocked(BlockReason),
}

impl RiskTier {
    /// Стабильный строковый дискриминант для ledger (`agent_actions.risk_tier`). Только верхний уровень
    /// тира (причина хранится отдельно/в diff_summary) — единый источник для SQL/чтений.
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskTier::Auto => "auto",
            RiskTier::Confirm(_) => "confirm",
            RiskTier::HardBlocked(_) => "hardblocked",
        }
    }
}

/// Контекст классификации. ТОЛЬКО факты, нужные для ДЕТЕРМИНИРОВАННОГО решения — никакого хэндла к ФС/БД
/// (чтобы classify оставался чистым). `root` нужен лишь как тип-маркер границы (лексическая проверка его
/// не канонизирует); `overwrite_threshold` — порог «крупной перезаписи» (NoteEdit → Confirm).
pub struct ClassifyCtx<'a> {
    /// Корень vault (тип-маркер границы; лексическая проверка ФС не трогает).
    pub root: &'a Path,
    /// Порог размера тела (байт) для NoteEdit: `> threshold` ⇒ Confirm(LargeOverwrite).
    pub overwrite_threshold: usize,
    /// **Фаза-3 (6b):** разрешено ли host-исполнение (`ai.shell_enable`). `false` → exec-таргеты
    /// HardBlocked(ShellDisabled). Vault-таргеты его ИГНОРИРУЮТ.
    pub shell_enable: bool,
    /// **Фаза-3 (6b):** доступна ли песочница СТРУКТУРНО (`sandbox_enabled` И Linux) — ПРЕДвычисляется
    /// вызывающим (classify остаётся чистой). `false` → exec-таргеты HardBlocked(SandboxUnavailable).
    pub sandbox_available: bool,
    /// **SL-7:** разрешено ли самообучение (`ai.skills.learning_enabled`). `false` → `SkillSave`
    /// HardBlocked(LearningDisabled). Vault/exec-таргеты его ИГНОРИРУЮТ.
    pub learning_enabled: bool,
    /// **SL-7:** сконфигурирован ли skills_root (`ai.agent_skills_dir` задан и канонизирован) —
    /// ПРЕДвычисляется вызывающим. `false` → `SkillSave` HardBlocked(SkillsRootUnconfigured).
    pub skills_root_configured: bool,
}

/// ЛЕКСИЧЕСКАЯ (без ФС) проверка конфайнмента vault-rel пути — fail-closed.
///
/// Возвращает `Err(BlockReason)` для НЕбезопасного пути, `Ok(())` для in-vault not-reserved. Строго
/// fail-closed: при любой неоднозначности — блок. Правила (НАДмножество canonicalize-границы):
///  - пустой путь / нет финального имени файла → [`BlockReason::EmptyPath`];
///  - абсолютный или root-anchored (`/x`, `C:\x`, `\x`) → [`BlockReason::PathEscape`] (как
///    `resolve_vault_path`: `is_absolute() || has_root()`, плюс мы режем явный root-prefix-компонент);
///  - любой компонент `..` (ParentDir) → [`BlockReason::PathEscape`] (traversal, лексически — ДО любой
///    канонизации, так что симлинк-обход тут неприменим: мы не следуем по ФС);
///  - любой компонент, начинающийся с `.` (`.nexus`, `.git`, любой dotfile/dotdir) →
///    [`BlockReason::ReservedPath`] (НАДмножество `.nexus`/`.git`: режем ВСЕ dot-компоненты —
///    fail-closed, чтобы будущий служебный dot-каталог не просочился).
///
/// `pub(crate)`: переиспользуется [`crate::sandbox::exec_child::resolve_cwd`] (SANDBOX-6c-2) как ЕДИНЫЙ
/// источник правила лексического конфайнмента vault-rel (не копия) для cwd exec-команд в песочнице.
pub(crate) fn path_confinement(rel: &str) -> Result<(), BlockReason> {
    if rel.trim().is_empty() {
        return Err(BlockReason::EmptyPath);
    }
    // КРОСС-ПЛАТФОРМЕННЫЙ fail-closed: backslash блокируем ВСЕГДА. На Unix `std::path` НЕ считает `\`
    // разделителем — `a\..\..\secret` распарсился бы как ОДИН компонент-имя, и `..`-проверка ниже его
    // НЕ поймала бы (обход traversal-фильтра), а `\windows\abs.md` прошёл бы как Auto, хотя на Windows
    // он root-anchored (побег). vault-rel всегда `/`-разделён (vault::to_unix) — backslash здесь
    // подозрителен по определению. Режем до парсинга, детерминированно на любой платформе.
    if rel.contains('\\') {
        return Err(BlockReason::PathEscape);
    }
    let p = Path::new(rel);
    // Абсолютный/root-anchored — как vault::resolve_vault_path (кросс-платформенно: Unix `/x`,
    // Windows `/x`/`\x`/`C:\x`). Это ПЕРВЫЙ рубеж; компонентный обход ниже — второй.
    if p.is_absolute() || p.has_root() {
        return Err(BlockReason::PathEscape);
    }
    let mut saw_name = false;
    for comp in p.components() {
        match comp {
            // `..` — побег вверх (лексически, без следования по ФС). Блок.
            Component::ParentDir => return Err(BlockReason::PathEscape),
            // Корневые/префиксные компоненты (на случай, если has_root() их не поймал на платформе).
            Component::RootDir | Component::Prefix(_) => return Err(BlockReason::PathEscape),
            // `.` — безвредный no-op компонент; пропускаем (не считаем именем файла).
            Component::CurDir => {}
            Component::Normal(os) => {
                let name = os.to_string_lossy();
                // ВСЕ dot-компоненты зарезервированы (.nexus/.git и любой иной служебный dot-вход) —
                // fail-closed надмножество. Имя «.» сюда не попадает (это CurDir).
                if name.starts_with('.') {
                    return Err(BlockReason::ReservedPath);
                }
                saw_name = true;
            }
        }
    }
    if !saw_name {
        // Путь состоял только из `.`-сегментов и т.п. — нет реального имени файла.
        return Err(BlockReason::EmptyPath);
    }
    Ok(())
}

/// PURE fail-closed классификация действия в тир риска. Детерминирована; БЕЗ файлового IO.
///
/// EXHAUSTIVE по [`ActionTarget`] (НЕТ `_ =>`): каждый вариант — явное решение. Контракт по вариантам:
///  - **NoteCreate**: путь in-vault & not-reserved → [`RiskTier::Auto`]; побег/резерв → HardBlocked.
///    Existence (цель уже есть) classify НЕ проверяет (это IO — забота `apply`/3c, который сделает
///    create over-existing ошибкой); classify решает по пути.
///  - **NoteEdit**: путь in-vault & not-reserved → размер тела `> overwrite_threshold` ?
///    [`RiskTier::Confirm`](LargeOverwrite) : [`RiskTier::Auto`]; побег/резерв → HardBlocked.
///    Размер берём из `content.len()` (None ⇒ 0 ⇒ Auto: пустая правка не крупная).
///  - **Frontmatter**: путь in-vault & not-reserved → [`RiskTier::Auto`] (хирургический одно-ключевой
///    патч под snapshot позже); побег/резерв → HardBlocked.
///
/// Эскалация безопасна: блок пути ВСЕГДА бьёт тир содержимого (сначала проверяем путь — побег у NoteEdit
/// не «понижается» до Confirm/Auto, а сразу HardBlocked).
pub fn classify(action: &Action, ctx: &ClassifyCtx) -> RiskTier {
    match &action.target {
        ActionTarget::NoteCreate { rel } => match path_confinement(rel) {
            Err(reason) => RiskTier::HardBlocked(reason),
            Ok(()) => RiskTier::Auto,
        },
        ActionTarget::NoteEdit { rel } => match path_confinement(rel) {
            Err(reason) => RiskTier::HardBlocked(reason),
            Ok(()) => {
                let size = action.content.as_ref().map(|c| c.len()).unwrap_or(0);
                if size > ctx.overwrite_threshold {
                    RiskTier::Confirm(ConfirmReason::LargeOverwrite)
                } else {
                    RiskTier::Auto
                }
            }
        },
        ActionTarget::Frontmatter { rel, .. } => match path_confinement(rel) {
            Err(reason) => RiskTier::HardBlocked(reason),
            Ok(()) => RiskTier::Auto,
        },
        // SL-7: авторство/перезапись SKILL.md — НИКОГДА Auto (агент не само-апрувит свои инструкции).
        ActionTarget::SkillSave { rel } => classify_skill_save(ctx, rel),
        // Фаза-3 host exec-таргеты — ЕДИНАЯ ветка (GitOp НЕ дробим на read-Auto, §5.3). НИКОГДА Auto.
        ActionTarget::ShellRun { .. }
        | ActionTarget::ProcessSpawn { .. }
        | ActionTarget::GitOp { .. } => classify_exec(ctx),
    }
}

/// Классификация SL-7 `SkillSave`. Precedence ЛОКИРОВАН (зеркало `classify_exec`): сначала
/// `learning_enabled` (самообучение не разрешено владельцем → `LearningDisabled`), затем наличие
/// skills_root (некуда писать → `SkillsRootUnconfigured`), затем ЛЕКСИЧЕСКИЙ конфайнмент `rel` внутри
/// skills_root ([`path_confinement`] — то же надмножество запретов: абсолют/`..`/dot-резерв), и ТОЛЬКО
/// при всём ОК → `Confirm`. **НИКОГДА `Auto`**: SKILL.md — это будущие инструкции агента, авторство
/// всегда человек-в-петле. `rel` тут трактуется так же лексически, как vault-rel (skills_root —
/// аналогичная база, канонизирующая проверка — в `apply_skill_save`, 6c).
fn classify_skill_save(ctx: &ClassifyCtx, rel: &str) -> RiskTier {
    if !ctx.learning_enabled {
        RiskTier::HardBlocked(BlockReason::LearningDisabled)
    } else if !ctx.skills_root_configured {
        RiskTier::HardBlocked(BlockReason::SkillsRootUnconfigured)
    } else {
        match path_confinement(rel) {
            Err(reason) => RiskTier::HardBlocked(reason),
            // path_confinement отсёк abs/`..`/dot/backslash; теперь ЛЕКСИЧЕСКИ требуем форму цели навыка.
            Ok(()) => match skill_target_shape(rel) {
                Err(reason) => RiskTier::HardBlocked(reason),
                Ok(()) => RiskTier::Confirm(ConfirmReason::SkillSaveRequiresApproval),
            },
        }
    }
}

/// ЛЕКСИЧЕСКАЯ форма цели `SkillSave` (предполагает уже пройденный [`path_confinement`]): РОВНО
/// `<имя>/SKILL.md` — два сегмента, второй == [`SKILL_FILE`], первый — непустое имя НЕ [`VENDOR_DIR`].
/// Иначе [`BlockReason::InvalidSkillTarget`]. Так keystone (а не доверенный-человек-апрувер и не
/// будущий apply) гарантирует: агент пишет лишь собственный `<name>/SKILL.md`, НИКОГДА не в `vendor/`
/// (hash-pinned неизменяемый) и не в чужие ресурсы/подкаталоги.
fn skill_target_shape(rel: &str) -> Result<(), BlockReason> {
    let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() != 2 || parts[1] != SKILL_FILE || parts[0] == VENDOR_DIR {
        return Err(BlockReason::InvalidSkillTarget);
    }
    Ok(())
}

/// Классификация Фаза-3 host exec-таргета. Порядок гейтов ЛОКИРОВАН (precedence): сначала `shell_enable`
/// (фича не разрешена владельцем → `ShellDisabled`), затем доступность песочницы (нет OS-границы →
/// `SandboxUnavailable`, block by-construction §9), и ТОЛЬКО при обоих ВКЛ → `Confirm` (исполнение
/// in-sandbox после апрува, 6c). **НИКОГДА `Auto`** — произвольное исполнение всегда человек-в-петле.
fn classify_exec(ctx: &ClassifyCtx) -> RiskTier {
    if !ctx.shell_enable {
        RiskTier::HardBlocked(BlockReason::ShellDisabled)
    } else if !ctx.sandbox_available {
        RiskTier::HardBlocked(BlockReason::SandboxUnavailable)
    } else {
        RiskTier::Confirm(ConfirmReason::ExecRequiresApproval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> (PathBuf, usize) {
        // root — лексический маркер; лексическая проверка его не канонизирует, поэтому несуществующий
        // путь годится (детерминизм без ФС). threshold — 100 байт для тестов порога.
        (PathBuf::from("/vault"), 100)
    }

    fn classify_create(rel: &str) -> RiskTier {
        let (root, t) = ctx();
        let c = ClassifyCtx {
            root: &root,
            overwrite_threshold: t,
            shell_enable: false,
            sandbox_available: false,
            learning_enabled: false,
            skills_root_configured: false,
        };
        classify(&Action::note_create(rel, "body"), &c)
    }

    fn classify_edit(rel: &str, body: &str) -> RiskTier {
        let (root, t) = ctx();
        let c = ClassifyCtx {
            root: &root,
            overwrite_threshold: t,
            shell_enable: false,
            sandbox_available: false,
            learning_enabled: false,
            skills_root_configured: false,
        };
        classify(&Action::note_edit(rel, body), &c)
    }

    fn classify_fm(rel: &str) -> RiskTier {
        let (root, t) = ctx();
        let c = ClassifyCtx {
            root: &root,
            overwrite_threshold: t,
            shell_enable: false,
            sandbox_available: false,
            learning_enabled: false,
            skills_root_configured: false,
        };
        classify(&Action::frontmatter(rel, "tags", "x"), &c)
    }

    // ── Таблица: каждый ActionTarget × {ok-auto, threshold→confirm, escape→block, reserved→block} ──

    /// NoteCreate, путь in-vault → Auto.
    #[test]
    fn note_create_in_vault_is_auto() {
        assert_eq!(classify_create("Notes/New.md"), RiskTier::Auto);
        assert_eq!(classify_create("a/b/c/deep.md"), RiskTier::Auto);
    }

    /// NoteCreate, побег (`..`/абсолют/root) → HardBlocked(PathEscape).
    #[test]
    fn note_create_escape_is_hardblocked() {
        assert_eq!(
            classify_create("../escape.md"),
            RiskTier::HardBlocked(BlockReason::PathEscape)
        );
        assert_eq!(
            classify_create("a/../../escape.md"),
            RiskTier::HardBlocked(BlockReason::PathEscape)
        );
        assert_eq!(
            classify_create("/tmp/abs.md"),
            RiskTier::HardBlocked(BlockReason::PathEscape)
        );
        // Windows-style backslash root.
        assert_eq!(
            classify_create("\\windows\\abs.md"),
            RiskTier::HardBlocked(BlockReason::PathEscape)
        );
        // КРИТИЧНО (кросс-платформа): backslash-traversal. На Unix `std::path` НЕ парсит `\` как
        // разделитель → без явного backslash-reject `a\..\..\secret` прошёл бы как ОДИН компонент-имя
        // (обход `..`-фильтра) и классифицировался бы Auto. Должен быть HardBlocked.
        assert_eq!(
            classify_create("a\\..\\..\\secret.md"),
            RiskTier::HardBlocked(BlockReason::PathEscape)
        );
    }

    /// NoteCreate, зарезервированный каталог (`.nexus`/`.git`/любой dot) → HardBlocked(ReservedPath).
    #[test]
    fn note_create_reserved_is_hardblocked() {
        assert_eq!(
            classify_create(".nexus/secret.md"),
            RiskTier::HardBlocked(BlockReason::ReservedPath)
        );
        assert_eq!(
            classify_create(".git/config"),
            RiskTier::HardBlocked(BlockReason::ReservedPath)
        );
        // Reserved может быть и ВНУТРИ пути, не только в начале.
        assert_eq!(
            classify_create("Notes/.git/hooks/x"),
            RiskTier::HardBlocked(BlockReason::ReservedPath)
        );
        // Любой иной dotfile тоже зарезервирован (fail-closed надмножество).
        assert_eq!(
            classify_create("Notes/.hidden.md"),
            RiskTier::HardBlocked(BlockReason::ReservedPath)
        );
    }

    /// NoteCreate, пустой путь → HardBlocked(EmptyPath).
    #[test]
    fn note_create_empty_is_hardblocked() {
        assert_eq!(
            classify_create(""),
            RiskTier::HardBlocked(BlockReason::EmptyPath)
        );
        assert_eq!(
            classify_create("   "),
            RiskTier::HardBlocked(BlockReason::EmptyPath)
        );
    }

    /// NoteEdit, in-vault & тело ≤ порога → Auto.
    #[test]
    fn note_edit_small_is_auto() {
        assert_eq!(classify_edit("Notes/N.md", "small"), RiskTier::Auto);
        // Ровно на пороге (== threshold) — НЕ крупная (строго `>`).
        let exactly = "x".repeat(100);
        assert_eq!(classify_edit("Notes/N.md", &exactly), RiskTier::Auto);
    }

    /// NoteEdit, in-vault & тело > порога → Confirm(LargeOverwrite).
    #[test]
    fn note_edit_large_is_confirm() {
        let big = "x".repeat(101);
        assert_eq!(
            classify_edit("Notes/N.md", &big),
            RiskTier::Confirm(ConfirmReason::LargeOverwrite)
        );
    }

    /// NoteEdit, побег/резерв БЬЁТ размер: даже КРУПНАЯ правка по побегу → HardBlocked, НЕ Confirm.
    /// (Регрессия против «понижения» опасного действия: путь проверяется ПЕРВЫМ.)
    #[test]
    fn note_edit_escape_beats_size() {
        let big = "x".repeat(10_000);
        assert_eq!(
            classify_edit("../escape.md", &big),
            RiskTier::HardBlocked(BlockReason::PathEscape)
        );
        assert_eq!(
            classify_edit(".git/config", &big),
            RiskTier::HardBlocked(BlockReason::ReservedPath)
        );
    }

    /// Frontmatter, in-vault → Auto.
    #[test]
    fn frontmatter_in_vault_is_auto() {
        assert_eq!(classify_fm("Notes/N.md"), RiskTier::Auto);
    }

    /// Frontmatter, побег/резерв → HardBlocked.
    #[test]
    fn frontmatter_escape_reserved_is_hardblocked() {
        assert_eq!(
            classify_fm("../escape.md"),
            RiskTier::HardBlocked(BlockReason::PathEscape)
        );
        assert_eq!(
            classify_fm(".nexus/x.md"),
            RiskTier::HardBlocked(BlockReason::ReservedPath)
        );
    }

    /// ПУРИТЕТ: один и тот же вход → один и тот же тир (детерминизм; вызов 1000× стабилен, нет ФС).
    #[test]
    fn classify_is_pure_deterministic() {
        let (root, t) = ctx();
        let c = ClassifyCtx {
            root: &root,
            overwrite_threshold: t,
            shell_enable: false,
            sandbox_available: false,
            learning_enabled: false,
            skills_root_configured: false,
        };
        let a = Action::note_edit("Notes/N.md", "x".repeat(200));
        let first = classify(&a, &c);
        for _ in 0..1000 {
            assert_eq!(classify(&a, &c), first, "classify недетерминирован");
        }
        assert_eq!(first, RiskTier::Confirm(ConfirmReason::LargeOverwrite));
    }

    /// `as_str` стабилен для ledger (auto|confirm|hardblocked).
    #[test]
    fn risk_tier_as_str_stable() {
        assert_eq!(RiskTier::Auto.as_str(), "auto");
        assert_eq!(
            RiskTier::Confirm(ConfirmReason::LargeOverwrite).as_str(),
            "confirm"
        );
        assert_eq!(
            RiskTier::HardBlocked(BlockReason::PathEscape).as_str(),
            "hardblocked"
        );
    }

    /// ДОКУМЕНТ-НАМЕРЕНИЕ (D4 keystone): `classify` матчит ActionTarget БЕЗ `_ =>`. Если добавить
    /// гипотетический вариант (например, `Shell`), компилятор СЛОМАЕТ match в classify — новый вид
    /// действия НЕ провалится молча в Auto через catch-all. Этот тест-зеркало демонстрирует, что
    /// exhaustive-матч над тем же набором вариантов компилируется только при покрытии ВСЕХ; добавление
    /// варианта в `ActionTarget` потребует ветки и здесь, и в `classify` (см. модульную доку).
    #[test]
    fn adding_variant_breaks_match() {
        fn must_be_exhaustive(t: &ActionTarget) -> &'static str {
            // Зеркало classify: НЕТ `_ =>`. Новый вариант ActionTarget сломает ОБА матча компиляцией.
            match t {
                ActionTarget::NoteCreate { .. } => "create",
                ActionTarget::NoteEdit { .. } => "edit",
                ActionTarget::Frontmatter { .. } => "fm",
                ActionTarget::SkillSave { .. } => "skill_save",
                ActionTarget::ShellRun { .. } => "shell",
                ActionTarget::ProcessSpawn { .. } => "process",
                ActionTarget::GitOp { .. } => "git",
            }
        }
        assert_eq!(
            must_be_exhaustive(&ActionTarget::NoteCreate { rel: "a.md".into() }),
            "create"
        );
        assert_eq!(
            must_be_exhaustive(&ActionTarget::NoteEdit { rel: "a.md".into() }),
            "edit"
        );
        assert_eq!(
            must_be_exhaustive(&ActionTarget::Frontmatter {
                rel: "a.md".into(),
                key: "k".into()
            }),
            "fm"
        );
    }

    // ── Фаза-3 (6b): classify exec-таргетов — НИКОГДА Auto (§5.3) ──
    fn classify_exec_t(
        target: ActionTarget,
        shell_enable: bool,
        sandbox_available: bool,
    ) -> RiskTier {
        let (root, t) = ctx();
        let c = ClassifyCtx {
            root: &root,
            overwrite_threshold: t,
            shell_enable,
            sandbox_available,
            learning_enabled: false,
            skills_root_configured: false,
        };
        classify(
            &Action {
                target,
                content: None,
                value: None,
            },
            &c,
        )
    }

    fn exec_targets() -> Vec<ActionTarget> {
        vec![
            ActionTarget::ShellRun {
                argv: vec!["ls".into()],
                cwd_rel: None,
            },
            ActionTarget::ProcessSpawn {
                program: "git".into(),
                args: vec![],
                cwd_rel: None,
            },
            ActionTarget::GitOp {
                op: "status".into(),
                args: vec![],
            },
        ]
    }

    /// KEYSTONE (§5.3): exec-таргет НИКОГДА не Auto — по ВСЕЙ сетке (shell_enable × sandbox_available) ×
    /// 3 варианта. Компилятор форсирует ВЕТКУ, этот тест форсирует её БЕЗОПАСНОСТЬ.
    #[test]
    fn exec_targets_never_auto() {
        for t in exec_targets() {
            for shell in [false, true] {
                for sandbox in [false, true] {
                    let tier = classify_exec_t(t.clone(), shell, sandbox);
                    assert!(
                        !matches!(tier, RiskTier::Auto),
                        "exec {t:?} (shell={shell}, sandbox={sandbox}) НЕ должен быть Auto, был {tier:?}"
                    );
                }
            }
        }
    }

    /// shell_enable=false → ВСЕГДА HardBlocked(ShellDisabled), приоритет ВПЕРЁД sandbox-гейта.
    #[test]
    fn exec_shell_disabled_precedence() {
        for t in exec_targets() {
            for sandbox in [false, true] {
                assert_eq!(
                    classify_exec_t(t.clone(), false, sandbox),
                    RiskTier::HardBlocked(BlockReason::ShellDisabled)
                );
            }
        }
    }

    /// shell_enable=true но песочница недоступна → HardBlocked(SandboxUnavailable) (block by-construction).
    #[test]
    fn exec_sandbox_unavailable() {
        for t in exec_targets() {
            assert_eq!(
                classify_exec_t(t.clone(), true, false),
                RiskTier::HardBlocked(BlockReason::SandboxUnavailable)
            );
        }
    }

    /// shell_enable=true + песочница доступна → Confirm(ExecRequiresApproval) (единственная рабочая ячейка).
    #[test]
    fn exec_enabled_is_confirm() {
        for t in exec_targets() {
            assert_eq!(
                classify_exec_t(t.clone(), true, true),
                RiskTier::Confirm(ConfirmReason::ExecRequiresApproval)
            );
        }
    }

    // ── SL-7: classify SkillSave — НИКОГДА Auto (зеркало exec) ──
    fn classify_skill(rel: &str, learning_enabled: bool, skills_root_configured: bool) -> RiskTier {
        let (root, t) = ctx();
        let c = ClassifyCtx {
            root: &root,
            overwrite_threshold: t,
            shell_enable: false,
            sandbox_available: false,
            learning_enabled,
            skills_root_configured,
        };
        classify(&Action::skill_save(rel, "BODY"), &c)
    }

    /// KEYSTONE: SkillSave НИКОГДА не Auto — по всей сетке (learning × root_configured × валидность пути).
    #[test]
    fn skill_save_never_auto() {
        for learning in [false, true] {
            for root_cfg in [false, true] {
                for rel in ["pdf/SKILL.md", "../escape/SKILL.md", ".nexus/x", ""] {
                    let tier = classify_skill(rel, learning, root_cfg);
                    assert!(
                        !matches!(tier, RiskTier::Auto),
                        "SkillSave(rel={rel}, learning={learning}, root={root_cfg}) НЕ Auto, был {tier:?}"
                    );
                }
            }
        }
    }

    /// learning_enabled=false → HardBlocked(LearningDisabled) ВПЕРЁД любых иных гейтов (даже валидный путь).
    #[test]
    fn skill_save_learning_disabled_precedence() {
        for root_cfg in [false, true] {
            assert_eq!(
                classify_skill("pdf/SKILL.md", false, root_cfg),
                RiskTier::HardBlocked(BlockReason::LearningDisabled)
            );
        }
    }

    /// learning ON но skills_root не сконфигурирован → HardBlocked(SkillsRootUnconfigured).
    #[test]
    fn skill_save_root_unconfigured() {
        assert_eq!(
            classify_skill("pdf/SKILL.md", true, false),
            RiskTier::HardBlocked(BlockReason::SkillsRootUnconfigured)
        );
    }

    /// learning ON + root сконфигурирован + валидный путь → Confirm(SkillSaveRequiresApproval).
    #[test]
    fn skill_save_enabled_valid_is_confirm() {
        assert_eq!(
            classify_skill("pdf/SKILL.md", true, true),
            RiskTier::Confirm(ConfirmReason::SkillSaveRequiresApproval)
        );
    }

    /// learning ON + root ON, но путь — побег/резерв → HardBlocked (путь бьёт Confirm).
    #[test]
    fn skill_save_bad_path_hardblocked() {
        assert_eq!(
            classify_skill("../escape/SKILL.md", true, true),
            RiskTier::HardBlocked(BlockReason::PathEscape)
        );
        assert_eq!(
            classify_skill(".nexus/sneaky/SKILL.md", true, true),
            RiskTier::HardBlocked(BlockReason::ReservedPath)
        );
        assert_eq!(
            classify_skill("", true, true),
            RiskTier::HardBlocked(BlockReason::EmptyPath)
        );
    }

    /// KEYSTONE-регрессия (ревью SL-7a+b, MAJOR): `vendor/`-неймспейс и любая форма ≠ `<имя>/SKILL.md` →
    /// HardBlocked(InvalidSkillTarget). Защищает hash-pinned вендоренные навыки от on-disk клоббера и
    /// запись чужих ресурсов/коллизий — ЛЕКСИЧЕСКИ в keystone, а не доверяясь апруверу/будущему apply.
    #[test]
    fn skill_save_vendor_and_bad_shape_hardblocked() {
        for rel in [
            "vendor/kepano/obsidian-markdown/SKILL.md", // вендор-неймспейс (и >2 сегментов)
            "vendor/SKILL.md",                          // первый сегмент == vendor
            "myskill/references/data.csv",              // 3 сегмента / не SKILL.md
            "myskill/notes.md",                         // 2 сегмента, но файл не SKILL.md
            "SKILL.md",                                 // 1 сегмент (нет имени навыка)
            "a/b/SKILL.md",                             // 3 сегмента
        ] {
            assert_eq!(
                classify_skill(rel, true, true),
                RiskTier::HardBlocked(BlockReason::InvalidSkillTarget),
                "rel={rel} должен быть InvalidSkillTarget"
            );
        }
        // А валидная форма по-прежнему Confirm (не зарегрессировали).
        assert_eq!(
            classify_skill("myskill/SKILL.md", true, true),
            RiskTier::Confirm(ConfirmReason::SkillSaveRequiresApproval)
        );
    }
}
