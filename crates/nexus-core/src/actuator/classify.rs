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

/// Причина жёсткой блокировки (HardBlocked) — действие НЕ исполняется ни авто, ни по подтверждению.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// Путь выходит за пределы vault (абсолютный/root-anchored или `..`-traversal).
    PathEscape,
    /// Путь ведёт в зарезервированный/служебный каталог (`.nexus`/`.git` и прочие dot-компоненты).
    ReservedPath,
    /// Пустой/невалидный rel-путь (нет имени файла, пустая строка).
    EmptyPath,
}

/// Причина, по которой действие требует ПОДТВЕРЖДЕНИЯ пользователя (Confirm) перед исполнением.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmReason {
    /// Перезапись существующей заметки телом крупнее порога (`overwrite_threshold` байт): большой
    /// блок-радиус потери данных → не авто, спросить.
    LargeOverwrite,
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
fn path_confinement(rel: &str) -> Result<(), BlockReason> {
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
        };
        classify(&Action::note_create(rel, "body"), &c)
    }

    fn classify_edit(rel: &str, body: &str) -> RiskTier {
        let (root, t) = ctx();
        let c = ClassifyCtx {
            root: &root,
            overwrite_threshold: t,
        };
        classify(&Action::note_edit(rel, body), &c)
    }

    fn classify_fm(rel: &str) -> RiskTier {
        let (root, t) = ctx();
        let c = ClassifyCtx {
            root: &root,
            overwrite_threshold: t,
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
}
