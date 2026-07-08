use super::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const VALID: &str =
    "---\nname: pdf-tools\ndescription: Work with PDF files\n---\n# PDF tools\n\nDo the thing.\n";

// ── parse_skill ────────────────────────────────────────────────────────────────────────────

/// Валидный SKILL.md (name+description+body) → Skill корректен. SKILL-3: tier выводится из пути
/// (top-level → TrustedLocal), license отсутствует (None).
#[test]
fn parse_valid_skill() {
    let s = parse_skill(VALID, "pdf-tools/SKILL.md").unwrap();
    assert_eq!(s.name, "pdf-tools");
    assert_eq!(s.description, "Work with PDF files");
    assert_eq!(s.rel_path, "pdf-tools/SKILL.md");
    assert_eq!(s.body, "# PDF tools\n\nDo the thing.\n");
    assert!(s.capabilities.is_empty());
    assert_eq!(s.tier, TrustTier::TrustedLocal, "top-level → TrustedLocal");
    assert_eq!(s.license, None, "лицензия не объявлена inline");
}

/// SKILL-3: vendored-путь → tier=Vendor уже на этапе parse (FS-валидация — в discover_skills).
#[test]
fn parse_vendored_path_is_vendor_tier() {
    let s = parse_skill(VALID, "vendor/kepano/x/SKILL.md").unwrap();
    assert_eq!(s.tier, TrustTier::Vendor);
}

/// SKILL-3: inline `license:` в frontmatter (само-декларация TrustedLocal) — захватывается.
#[test]
fn parse_inline_license_captured() {
    let content = "---\nname: x\ndescription: d\nlicense: Apache-2.0\n---\nbody\n";
    let s = parse_skill(content, "x/SKILL.md").unwrap();
    assert_eq!(s.license, Some("Apache-2.0".to_string()));
}

/// kepano-скилл: frontmatter ТОЛЬКО name+description, без metadata.nexus.* → грузится без проблем.
#[test]
fn parse_kepano_skill_no_nexus_fields() {
    let content =
        "---\nname: daily-note\ndescription: Create a daily note from template\n---\nBody here.\n";
    let s = parse_skill(content, "daily-note/SKILL.md").unwrap();
    assert_eq!(s.name, "daily-note");
    assert_eq!(s.description, "Create a daily note from template");
    assert!(s.capabilities.is_empty());
}

/// Краевые кавычки значений снимаются (edge-stripper), last-key-wins.
#[test]
fn parse_strips_quotes_and_last_key_wins() {
    let content = "---\nname: \"quoted\"\ndescription: 'single quoted'\nname: final\n---\nbody\n";
    let s = parse_skill(content, "x/SKILL.md").unwrap();
    assert_eq!(s.name, "final"); // last-key-wins
    assert_eq!(s.description, "single quoted");
}

// ── malformed → hard error (not silent) ──────────────────────────────────────────────────────

#[test]
fn parse_missing_name() {
    let content = "---\ndescription: has desc only\n---\nbody\n";
    assert_eq!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::MissingName)
    );
}

#[test]
fn parse_missing_description() {
    let content = "---\nname: only-name\n---\nbody\n";
    assert_eq!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::MissingDescription)
    );
}

/// `name:` присутствует, но значение пустое → MissingName (пустое ≠ есть).
#[test]
fn parse_empty_name_value() {
    let content = "---\nname:\ndescription: d\n---\nbody\n";
    assert_eq!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::MissingName)
    );
}

/// `name: ""` (пустые кавычки) после edge-stripper → пусто → MissingName.
#[test]
fn parse_empty_quoted_name_value() {
    let content = "---\nname: \"\"\ndescription: d\n---\nbody\n";
    assert_eq!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::MissingName)
    );
}

/// Нет frontmatter вовсе → BadFrontmatter (НЕ тихо «нет имени»).
#[test]
fn parse_no_frontmatter() {
    let content = "# Just a heading\n\nNo frontmatter at all.\n";
    assert_eq!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadFrontmatter)
    );
}

/// Открывающий `---` без закрывающего (unterminated) → BadFrontmatter.
#[test]
fn parse_unterminated_frontmatter() {
    let content = "---\nname: x\ndescription: y\nbody continues without closing fence\n";
    assert_eq!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadFrontmatter)
    );
}

// ── adversarial frontmatter ──────────────────────────────────────────────────────────────────

/// name с разделителем пути (`../`) → BadName (нельзя использовать как путь).
#[test]
fn parse_name_with_traversal_rejected() {
    let content = "---\nname: ../evil\ndescription: d\n---\nbody\n";
    assert!(matches!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadName(_))
    ));
}

/// name с прямым слэшем → BadName.
#[test]
fn parse_name_with_slash_rejected() {
    let content = "---\nname: a/b\ndescription: d\n---\nbody\n";
    assert!(matches!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadName(_))
    ));
}

/// name с backslash → BadName (Windows-разделитель).
#[test]
fn parse_name_with_backslash_rejected() {
    let content = "---\nname: a\\b\ndescription: d\n---\nbody\n";
    assert!(matches!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadName(_))
    ));
}

/// name с control-символом (tab внутри значения через кавычки) → BadName.
#[test]
fn parse_name_with_control_char_rejected() {
    // \t внутри кавычек переживёт edge-stripper (он тримит только КРАЕВЫЕ).
    let content = "---\nname: \"a\tb\"\ndescription: d\n---\nbody\n";
    assert!(matches!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadName(_))
    ));
}

/// Огромное имя → BadName (анти-«huge»).
#[test]
fn parse_huge_name_rejected() {
    let big = "x".repeat(500);
    let content = format!("---\nname: {big}\ndescription: d\n---\nbody\n");
    assert!(matches!(
        parse_skill(&content, "x/SKILL.md"),
        Err(SkillError::BadName(_))
    ));
}

// ── capabilities capture (no enforcement) ────────────────────────────────────────────────────

/// Инлайн-список capabilities → захвачен в Skill.
#[test]
fn parse_capabilities_inline_list() {
    let content = "---\nname: x\ndescription: d\ncapabilities: [read, write, net]\n---\nbody\n";
    let s = parse_skill(content, "x/SKILL.md").unwrap();
    assert_eq!(s.capabilities, vec!["read", "write", "net"]);
}

/// Блочный список capabilities → захвачен.
#[test]
fn parse_capabilities_block_list() {
    let content = "---\nname: x\ndescription: d\ncapabilities:\n  - read\n  - write\n---\nbody\n";
    let s = parse_skill(content, "x/SKILL.md").unwrap();
    assert_eq!(s.capabilities, vec!["read", "write"]);
}

/// `allowed-tools:` (синоним по стандарту) тоже захватывается.
#[test]
fn parse_capabilities_allowed_tools_alias() {
    let content = "---\nname: x\ndescription: d\nallowed-tools: [Bash, Read]\n---\nbody\n";
    let s = parse_skill(content, "x/SKILL.md").unwrap();
    assert_eq!(s.capabilities, vec!["Bash", "Read"]);
}

/// Поля capabilities нет → пустой Vec, не ошибка.
#[test]
fn parse_capabilities_absent_is_ok() {
    let s = parse_skill(VALID, "x/SKILL.md").unwrap();
    assert!(s.capabilities.is_empty());
}

/// Битый capabilities (скаляр вместо списка) → BadCapabilities (fail-closed).
#[test]
fn parse_capabilities_scalar_is_error() {
    let content = "---\nname: x\ndescription: d\ncapabilities: justone\n---\nbody\n";
    assert!(matches!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadCapabilities(_))
    ));
}

/// Битый capabilities (инлайн-список без `]`) → BadCapabilities.
#[test]
fn parse_capabilities_unterminated_inline_is_error() {
    let content = "---\nname: x\ndescription: d\ncapabilities: [a, b\n---\nbody\n";
    assert!(matches!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadCapabilities(_))
    ));
}

/// Битый capabilities (пустой элемент `[a, , b]`) → BadCapabilities.
#[test]
fn parse_capabilities_empty_element_is_error() {
    let content = "---\nname: x\ndescription: d\ncapabilities: [a, , b]\n---\nbody\n";
    assert!(matches!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadCapabilities(_))
    ));
}

/// Объявленное-но-пустое поле capabilities → BadCapabilities (не «тихо пусто»).
#[test]
fn parse_capabilities_declared_empty_is_error() {
    let content = "---\nname: x\ndescription: d\ncapabilities:\n---\nbody\n";
    assert!(matches!(
        parse_skill(content, "x/SKILL.md"),
        Err(SkillError::BadCapabilities(_))
    ));
}

// ── discovery: helpers ───────────────────────────────────────────────────────────────────────

fn write_skill(root: &Path, dir: &str, name: &str, desc: &str) {
    let d = root.join(dir);
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join(SKILL_FILE),
        format!("---\nname: {name}\ndescription: {desc}\n---\nBody of {name}.\n"),
    )
    .unwrap();
}

// ── discovery: happy path ────────────────────────────────────────────────────────────────────

#[test]
fn discover_loads_per_dir_skills() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_skill(root, "alpha", "alpha", "first skill");
    write_skill(root, "beta", "beta", "second skill");

    let cat = discover_skills(root);
    assert_eq!(cat.len(), 2);
    assert!(cat.errors().is_empty());
    assert_eq!(cat.get("alpha").unwrap().description, "first skill");
    assert_eq!(cat.get("beta").unwrap().rel_path, "beta/SKILL.md");
}

/// Плоская раскладка `<skills_dir>/<name>.md` тоже поддержана.
#[test]
fn discover_loads_flat_md() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("flat.md"),
        "---\nname: flat-skill\ndescription: a flat one\n---\nbody\n",
    )
    .unwrap();
    let cat = discover_skills(root);
    assert_eq!(cat.len(), 1);
    assert_eq!(cat.get("flat-skill").unwrap().rel_path, "flat.md");
}

// ── discovery: malformed visible (not swallowed) ─────────────────────────────────────────────

/// Каталог с битым + валидным скиллом: валидный загружен, битый ВИДИМ в errors (не проглочен).
#[test]
fn discover_reports_malformed_keeps_valid() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_skill(root, "good", "good", "the good one");
    // битый: нет description
    let bad = root.join("bad");
    fs::create_dir_all(&bad).unwrap();
    fs::write(bad.join(SKILL_FILE), "---\nname: bad\n---\nbody\n").unwrap();

    let cat = discover_skills(root);
    assert_eq!(cat.len(), 1);
    assert!(cat.get("good").is_some());
    assert_eq!(cat.errors().len(), 1);
    assert_eq!(cat.errors()[0].0, "bad/SKILL.md");
    assert_eq!(cat.errors()[0].1, SkillError::MissingDescription);
}

// ── discovery: single-def duplicate ──────────────────────────────────────────────────────────

/// Два скилла объявили одно `name` → первый остаётся, второй — DuplicateName в errors.
#[test]
fn discover_duplicate_name_surfaced() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // Оба объявляют name: dup. Каталоги a/ и b/ → a сортируется первым → b конфликтует.
    write_skill(root, "a", "dup", "from a");
    write_skill(root, "b", "dup", "from b");

    let cat = discover_skills(root);
    assert_eq!(cat.len(), 1);
    assert_eq!(cat.get("dup").unwrap().description, "from a"); // первый победил
    assert_eq!(cat.errors().len(), 1);
    assert_eq!(cat.errors()[0].0, "b/SKILL.md");
    assert_eq!(
        cat.errors()[0].1,
        SkillError::DuplicateName("dup".to_string())
    );
}

// ── discovery: path-scope (symlink escape) ───────────────────────────────────────────────────

/// SKILL.md через симлинк-каталог, указывающий ВНЕ skills_dir → НЕ загружен (no escape).
#[cfg(unix)]
#[test]
fn discover_rejects_symlinked_dir_escape() {
    use std::os::unix::fs::symlink;

    let outside = TempDir::new().unwrap();
    // реальный скилл ВНЕ skills_dir
    write_skill(outside.path(), "evil", "evil", "should not load");

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_skill(root, "good", "good", "legit");
    // симлинк-каталог внутри skills_dir, указывающий на каталог-скилл снаружи
    symlink(outside.path().join("evil"), root.join("link-to-evil")).unwrap();

    let cat = discover_skills(root);
    // evil НЕ должен попасть в каталог (симлинк-каталог не обходим)
    assert!(
        cat.get("evil").is_none(),
        "симлинк-каталог наружу загрузился!"
    );
    assert_eq!(cat.len(), 1);
    assert!(cat.get("good").is_some());
}

/// Плоский `<name>.md`-симлинк, указывающий на файл ВНЕ skills_dir → НЕ загружен.
#[cfg(unix)]
#[test]
fn discover_rejects_symlinked_file_escape() {
    use std::os::unix::fs::symlink;

    let outside = TempDir::new().unwrap();
    let outside_skill = outside.path().join("secret.md");
    fs::write(
        &outside_skill,
        "---\nname: secret\ndescription: outside\n---\nbody\n",
    )
    .unwrap();

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_skill(root, "good", "good", "legit");
    symlink(&outside_skill, root.join("secret.md")).unwrap();

    let cat = discover_skills(root);
    assert!(
        cat.get("secret").is_none(),
        "симлинк-файл наружу загрузился!"
    );
    assert_eq!(cat.len(), 1);
    assert!(cat.get("good").is_some());
}

/// РЕАЛЬНЫЙ подкаталог внутри skills_dir, но его SKILL.md — симлинк на файл СНАРУЖИ.
/// Здесь symlink_metadata подкаталога говорит `is_dir`, `skill_md.is_file()` (следует симлинку)
/// тоже true → файл становится кандидатом. Ловит ВТОРОЙ рубеж: canonicalize+starts_with →
/// PathEscape (а не загрузка наружного контента). Явно проверяем, что бэкстоп не дыра.
#[cfg(unix)]
#[test]
fn discover_rejects_inner_dir_with_symlinked_skill_md_escape() {
    use std::os::unix::fs::symlink;

    let outside = TempDir::new().unwrap();
    let outside_skill = outside.path().join("payload.md");
    fs::write(
        &outside_skill,
        "---\nname: payload\ndescription: outside payload\n---\nbody\n",
    )
    .unwrap();

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_skill(root, "good", "good", "legit");
    // реальный каталог внутри, но SKILL.md в нём — симлинк наружу
    let inner = root.join("sneaky");
    fs::create_dir_all(&inner).unwrap();
    symlink(&outside_skill, inner.join(SKILL_FILE)).unwrap();

    let cat = discover_skills(root);
    assert!(
        cat.get("payload").is_none(),
        "SKILL.md-симлинк наружу из реального подкаталога загрузился!"
    );
    assert_eq!(cat.len(), 1);
    assert!(cat.get("good").is_some());
    // бэкстоп сработал именно как PathEscape (не Io/тихий пропуск)
    let escape = cat
        .errors()
        .iter()
        .find(|(rel, _)| rel == "sneaky/SKILL.md");
    assert_eq!(
        escape.map(|(_, e)| e),
        Some(&SkillError::PathEscape),
        "ожидался видимый PathEscape по sneaky/SKILL.md, errors={:?}",
        cat.errors()
    );
}

// ── discovery: edge cases ────────────────────────────────────────────────────────────────────

/// skills_dir не существует → пустой каталог + одна io-ошибка (а не паника).
#[test]
fn discover_missing_dir_is_io_error_not_panic() {
    let tmp = TempDir::new().unwrap();
    let missing: PathBuf = tmp.path().join("does-not-exist");
    let cat = discover_skills(&missing);
    assert!(cat.is_empty());
    assert_eq!(cat.errors().len(), 1);
    assert!(matches!(cat.errors()[0].1, SkillError::Io(_)));
}

/// Пустой skills_dir → пустой каталог, без ошибок.
#[test]
fn discover_empty_dir() {
    let tmp = TempDir::new().unwrap();
    let cat = discover_skills(tmp.path());
    assert!(cat.is_empty());
    assert!(cat.errors().is_empty());
}

/// Каталог без SKILL.md (просто папка) — пропущен молча (это не скилл), без ошибки.
#[test]
fn discover_dir_without_skill_md_skipped() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("not-a-skill")).unwrap();
    write_skill(root, "real", "real", "the real one");
    let cat = discover_skills(root);
    assert_eq!(cat.len(), 1);
    assert!(cat.get("real").is_some());
    assert!(cat.errors().is_empty());
}

// ── SKILL-2 tier 1: catalog_block (меню name+description, фенсен, бюджетирован) ─────────────────

/// Каталог из тела VALID: catalog_block перечисляет name+description, обёрнут маркером, и НЕ
/// содержит ТЕЛА (тело — tier 2). Это «меню», не инструкции.
#[test]
fn catalog_block_lists_names_descriptions_not_bodies() {
    // VALID-тело: "# PDF tools\n\nDo the thing.\n" — это body, его в меню быть НЕ должно.
    let cat = {
        let mut c = SkillCatalog::default();
        c.skills
            .push(parse_skill(VALID, "pdf-tools/SKILL.md").unwrap());
        c.skills.push(
            parse_skill(
                "---\nname: daily\ndescription: Create a daily note\n---\nSECRET-BODY-TEXT\n",
                "daily/SKILL.md",
            )
            .unwrap(),
        );
        c
    };
    let marker = "⟦TESTMARK⟧";
    let block = cat.catalog_block(marker).expect("непустой каталог → блок");
    // name+description присутствуют.
    assert!(block.contains("pdf-tools"), "имя скилла: {block}");
    assert!(block.contains("Work with PDF files"), "описание: {block}");
    assert!(block.contains("daily"), "второй скилл");
    // ТЕЛА скиллов НЕТ (tier-1 — только меню).
    assert!(
        !block.contains("Do the thing"),
        "тело pdf-tools НЕ в tier-1 меню: {block}"
    );
    assert!(
        !block.contains("SECRET-BODY-TEXT"),
        "тело daily НЕ в tier-1 меню: {block}"
    );
    // Фенсен маркером (на обоих концах каждого пункта).
    assert!(block.contains(marker), "блок обёрнут маркером");
    // Модели сказано: активировать через activate_skill (а не выполнять как инструкции).
    assert!(
        block.contains("activate_skill"),
        "указание на tier-2 инструмент"
    );
}

/// Пустой каталог → None (нечего инжектить).
#[test]
fn catalog_block_empty_is_none() {
    let cat = SkillCatalog::default();
    assert!(cat.catalog_block("⟦m⟧").is_none());
}

/// Многострочное/управляющее description сворачивается в одну строку: недоверенный текст не
/// «рвёт» формат меню (контент остаётся между маркерами = ДАННЫЕ, но пункт — однострочник).
#[test]
fn catalog_block_collapses_multiline_description() {
    let mut cat = SkillCatalog::default();
    cat.skills.push(Skill {
        name: "evil".into(),
        description: "line1\nline2\tx\rz".into(),
        rel_path: "evil/SKILL.md".into(),
        body: String::new(),
        capabilities: Vec::new(),
        tier: TrustTier::TrustedLocal,
        license: None,
    });
    let block = cat.catalog_block("⟦m⟧").unwrap();
    // Управляющие символы из description свёрнуты в пробелы — на одной строке.
    assert!(
        block.contains("evil: line1 line2 x z"),
        "однострочник: {block}"
    );
    assert!(
        !block.contains("line1\nline2"),
        "нет сырого переноса в пункте"
    );
}

/// Бюджет: МНОГО скиллов → меню усечено до CATALOG_MAX_ENTRIES с пометкой «…ещё N», а не
/// безграничный список. И длинное description обрезано по символам.
#[test]
fn catalog_block_is_budget_capped() {
    let mut cat = SkillCatalog::default();
    // CATALOG_MAX_ENTRIES + 7 скиллов → должно быть усечено.
    let n = CATALOG_MAX_ENTRIES + 7;
    for i in 0..n {
        cat.skills.push(Skill {
            name: format!("skill-{i}"),
            description: "d".repeat(CATALOG_DESC_MAX_CHARS + 100), // заведомо длинное
            rel_path: format!("skill-{i}/SKILL.md"),
            body: String::new(),
            capabilities: Vec::new(),
            tier: TrustTier::TrustedLocal,
            license: None,
        });
    }
    let block = cat.catalog_block("⟦m⟧").unwrap();
    // Показано РОВНО CATALOG_MAX_ENTRIES пунктов (считаем по уникальным именам в блоке).
    let shown = (0..n)
        .filter(|i| block.contains(&format!("skill-{i}:")))
        .count();
    assert_eq!(shown, CATALOG_MAX_ENTRIES, "меню усечено до капа");
    assert!(
        block.contains("…ещё 7 скиллов"),
        "явная пометка усечения: {block}"
    );
    // description обрезан: «…» присутствует, и нет полной 300-символьной строки «d».
    assert!(block.contains('…'), "длинное описание усечено по символам");
    assert!(
        !block.contains(&"d".repeat(CATALOG_DESC_MAX_CHARS + 1)),
        "полное длинное описание НЕ попало в меню"
    );
}

// ── SKILL-2 tier 3: resolve_skill_resource (конфайн в подкаталог скилла) ────────────────────────

/// Ресурс ВНУТРИ каталога скилла → резолвится в канонический путь под этим каталогом.
#[test]
fn resolve_resource_inside_skill_ok() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let skill_dir = root.join("pdf");
    fs::create_dir_all(skill_dir.join("assets")).unwrap();
    fs::write(skill_dir.join("SKILL.md"), VALID).unwrap();
    fs::write(skill_dir.join("assets/data.txt"), "RESOURCE").unwrap();

    let got = resolve_skill_resource(&root, "pdf/SKILL.md", "assets/data.txt").unwrap();
    assert!(
        got.starts_with(&skill_dir),
        "ресурс внутри каталога скилла: {got:?}"
    );
    assert_eq!(fs::read_to_string(&got).unwrap(), "RESOURCE");
}

/// `..`-traversal в ИМЯ другого скилла → PathEscape (не читаем чужой скилл).
#[test]
fn resolve_resource_traversal_to_sibling_rejected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("a")).unwrap();
    fs::write(root.join("a/SKILL.md"), VALID).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();
    fs::write(root.join("b/secret.txt"), "OTHER-SKILL-SECRET").unwrap();

    // Скилл `a` пытается прочитать ресурс скилла `b` через `..`.
    let r = resolve_skill_resource(&root, "a/SKILL.md", "../b/secret.txt");
    assert_eq!(r, Err(SkillError::PathEscape), "чужой скилл недоступен");
}

/// `..`-traversal ВЫШЕ skills_root → PathEscape (не выходим в vault/ФС).
#[test]
fn resolve_resource_traversal_above_root_rejected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("skills");
    fs::create_dir_all(root.join("a")).unwrap();
    fs::write(root.join("a/SKILL.md"), VALID).unwrap();
    // Файл ВНЕ skills_root (рядом, на уровень выше).
    fs::write(tmp.path().join("outside.txt"), "OUTSIDE").unwrap();
    let root = root.canonicalize().unwrap();

    let r = resolve_skill_resource(&root, "a/SKILL.md", "../../outside.txt");
    assert_eq!(
        r,
        Err(SkillError::PathEscape),
        "выход выше root заблокирован"
    );
}

/// Абсолютный путь ресурса → PathEscape (без canonicalize-гонки).
#[test]
fn resolve_resource_absolute_rejected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("a")).unwrap();
    fs::write(root.join("a/SKILL.md"), VALID).unwrap();
    assert_eq!(
        resolve_skill_resource(&root, "a/SKILL.md", "/etc/passwd"),
        Err(SkillError::PathEscape),
        "абсолютный путь заблокирован"
    );
}

/// Симлинк ВНУТРИ каталога скилла, указывающий НАРУЖУ → PathEscape (canonicalize+starts_with).
#[cfg(unix)]
#[test]
fn resolve_resource_symlink_escape_rejected() {
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let skill_dir = root.join("a");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), VALID).unwrap();
    // Секрет снаружи skills_root + симлинк на него внутри каталога скилла.
    let secret = tmp.path().join("secret.txt");
    fs::write(&secret, "SYMLINK-LEAK").unwrap();
    symlink(&secret, skill_dir.join("leak.txt")).unwrap();

    let r = resolve_skill_resource(&root, "a/SKILL.md", "leak.txt");
    assert_eq!(
        r,
        Err(SkillError::PathEscape),
        "симлинк наружу заблокирован"
    );
}

/// Несуществующий ресурс → Io (видимая ошибка, не паника/не «пусто»).
#[test]
fn resolve_resource_missing_is_io() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("a")).unwrap();
    fs::write(root.join("a/SKILL.md"), VALID).unwrap();
    assert!(matches!(
        resolve_skill_resource(&root, "a/SKILL.md", "nope.txt"),
        Err(SkillError::Io(_))
    ));
}

// ── SKILL-3: вендоринг discovery + manifest/license/hash валидация ───────────────────────────────

/// Считает sha256-hex содержимого (зеркало loader-side `sha256_hex`) — для синтез-фикстур.
fn sha256_of(content: &str) -> String {
    sha256_hex(content.as_bytes())
}

/// Пишет минимальный валидный SKILL.md в `<root>/vendor/<bundle>/<skill>/SKILL.md` и возвращает
/// его содержимое (чтобы тест мог запинить корректный хэш).
fn write_vendored_skill(root: &Path, bundle: &str, skill: &str, name: &str) -> String {
    let d = root.join(VENDOR_DIR).join(bundle).join(skill);
    fs::create_dir_all(&d).unwrap();
    let content =
        format!("---\nname: {name}\ndescription: vendored {name}\n---\nBody of {name}.\n");
    fs::write(d.join(SKILL_FILE), &content).unwrap();
    content
}

/// Пишет vendor.lock-манифест для bundle'а с заданными license + (rel_path, sha256)-пинами.
fn write_manifest(root: &Path, bundle: &str, license: &str, files: &[(&str, &str)]) {
    let files_json: Vec<String> = files
        .iter()
        .map(|(rp, sha)| format!("{{\"rel_path\":\"{rp}\",\"sha256\":\"{sha}\"}}"))
        .collect();
    let manifest = format!(
        "{{\"bundle\":\"{bundle}\",\"source\":\"test\",\"commit\":\"deadbeef\",\
         \"license\":\"{license}\",\"files\":[{}]}}",
        files_json.join(",")
    );
    fs::write(
        root.join(VENDOR_DIR).join(bundle).join(VENDOR_LOCK_FILE),
        manifest,
    )
    .unwrap();
}

/// Fail-safe: vendor-корень детектится РЕГИСТРОЗАВИСИМО (lowercase `vendor`). Каталог `Vendor/`
/// (заглавная) НЕ распознаётся как vendor-корень → его вложенные скиллы НЕ обнаруживаются (обычный
/// верхнеуровневый скан ищет `Vendor/SKILL.md`, которого нет). Ключевое: НЕТ обхода валидации —
/// vendored-скилл под нестандартным регистром просто НЕ грузится (а не грузится как TrustedLocal
/// без hash-pin). Документирует «case-sensitive by design» из adversarial-ревью.
#[test]
fn discover_uppercase_vendor_dir_is_not_loaded_no_bypass() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // Скилл под `Vendor/acme/tool/SKILL.md` (заглавная V), БЕЗ манифеста.
    let d = root.join("Vendor").join("acme").join("tool");
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join(SKILL_FILE),
        "---\nname: sneaky\ndescription: tries to dodge vendor validation\n---\nBody.\n",
    )
    .unwrap();

    let cat = discover_skills(root);
    // НЕ загружен (вложенная раскладка под нераспознанным vendor-корнем не обнаруживается)…
    assert!(
        cat.get("sneaky").is_none(),
        "vendored-скилл под `Vendor/` (заглавная) НЕ должен грузиться (нет обхода валидации)"
    );
    // …и не «протёк» как TrustedLocal без hash-pin: его попросту нет в каталоге.
    assert!(
        cat.skills().iter().all(|s| s.name != "sneaky"),
        "нет скилла `sneaky` ни в каком tier"
    );
}

/// Discovery находит вендоренный скилл (на уровень глубже): rel_path остаётся относительным
/// skills_dir; tier=Vendor; license из манифеста; чистая загрузка.
#[test]
fn discover_finds_vendored_skill_nested() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let content = write_vendored_skill(root, "acme", "tool", "vtool");
    let sha = sha256_of(&content);
    write_manifest(root, "acme", "MIT", &[("tool/SKILL.md", &sha)]);

    let cat = discover_skills(root);
    assert!(cat.errors().is_empty(), "чисто: {:?}", cat.errors());
    let s = cat
        .get("vtool")
        .expect("вендоренный скилл найден на уровень глубже");
    assert_eq!(
        s.rel_path, "vendor/acme/tool/SKILL.md",
        "rel отн. skills_dir"
    );
    assert_eq!(s.tier, TrustTier::Vendor);
    assert_eq!(s.license, Some("MIT".to_string()), "license из vendor.lock");
}

/// Top-level И vendored скиллы соседствуют: оба грузятся (раскладка SKILL-1 не сломана).
#[test]
fn discover_top_level_and_vendored_coexist() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_skill(root, "local", "local", "trusted-local one");
    let content = write_vendored_skill(root, "acme", "tool", "vtool");
    write_manifest(
        root,
        "acme",
        "MIT",
        &[("tool/SKILL.md", &sha256_of(&content))],
    );

    let cat = discover_skills(root);
    assert!(cat.errors().is_empty(), "{:?}", cat.errors());
    assert_eq!(cat.len(), 2);
    assert_eq!(cat.get("local").unwrap().tier, TrustTier::TrustedLocal);
    assert_eq!(cat.get("vtool").unwrap().tier, TrustTier::Vendor);
}

/// Vendored-скилл БЕЗ vendor.lock → MissingManifest (видим в errors, НЕ загружен).
#[test]
fn discover_vendored_without_manifest_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_vendored_skill(root, "acme", "tool", "vtool");
    // намеренно НЕ пишем vendor.lock.

    let cat = discover_skills(root);
    assert!(cat.get("vtool").is_none(), "без манифеста — не загружен");
    let err = cat
        .errors()
        .iter()
        .find(|(rel, _)| rel == "vendor/acme/tool/SKILL.md");
    assert!(
        matches!(err.map(|(_, e)| e), Some(SkillError::MissingManifest(_))),
        "ожидался MissingManifest, errors={:?}",
        cat.errors()
    );
}

/// Vendored-bundle с ПУСТОЙ license в манифесте → MissingLicense (не загружен).
#[test]
fn discover_vendored_missing_license_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let content = write_vendored_skill(root, "acme", "tool", "vtool");
    // license="" — обязательное поле пусто.
    write_manifest(root, "acme", "", &[("tool/SKILL.md", &sha256_of(&content))]);

    let cat = discover_skills(root);
    assert!(cat.get("vtool").is_none());
    let err = cat
        .errors()
        .iter()
        .find(|(rel, _)| rel == "vendor/acme/tool/SKILL.md");
    assert_eq!(
        err.map(|(_, e)| e),
        Some(&SkillError::MissingLicense),
        "errors={:?}",
        cat.errors()
    );
}

/// Vendored SKILL.md с НЕсовпадающим pin-хэшем (tamper) → HashMismatch (не загружен).
#[test]
fn discover_vendored_hash_mismatch_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_vendored_skill(root, "acme", "tool", "vtool");
    // Пин на ЗАВЕДОМО неверный хэш (как будто файл подменили после пиннинга).
    write_manifest(root, "acme", "MIT", &[("tool/SKILL.md", &"0".repeat(64))]);

    let cat = discover_skills(root);
    assert!(cat.get("vtool").is_none(), "tamper → не загружен");
    let err = cat
        .errors()
        .iter()
        .find(|(rel, _)| rel == "vendor/acme/tool/SKILL.md");
    assert!(
        matches!(err.map(|(_, e)| e), Some(SkillError::HashMismatch(_))),
        "ожидался HashMismatch, errors={:?}",
        cat.errors()
    );
}

/// Vendored SKILL.md, для которого в `files` НЕТ записи (pin отсутствует) → HashMismatch.
#[test]
fn discover_vendored_pin_missing_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_vendored_skill(root, "acme", "tool", "vtool");
    // Манифест есть, license есть, но files[] не содержит записи для tool/SKILL.md.
    write_manifest(root, "acme", "MIT", &[("other/SKILL.md", &"a".repeat(64))]);

    let cat = discover_skills(root);
    assert!(cat.get("vtool").is_none(), "нет пина → не загружен");
    let err = cat
        .errors()
        .iter()
        .find(|(rel, _)| rel == "vendor/acme/tool/SKILL.md");
    assert!(
        matches!(err.map(|(_, e)| e), Some(SkillError::HashMismatch(_))),
        "ожидался HashMismatch (нет записи), errors={:?}",
        cat.errors()
    );
}

/// Off-bundle: vendored SKILL.md — симлинк на файл СНАРУЖИ skills_dir → path-scope бэкстоп ловит
/// PathEscape ДО manifest-валидации (наружный контент не грузится). Регрессия SKILL-1-защиты на
/// vendor-уровне.
#[cfg(unix)]
#[test]
fn discover_vendored_offbundle_symlink_escape() {
    use std::os::unix::fs::symlink;
    let outside = TempDir::new().unwrap();
    let outside_skill = outside.path().join("payload.md");
    fs::write(
        &outside_skill,
        "---\nname: payload\ndescription: outside\n---\nbody\n",
    )
    .unwrap();

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // Реальный vendor/<bundle>/<skill>/, но SKILL.md в нём — симлинк наружу.
    let skill_dir = root.join(VENDOR_DIR).join("acme").join("tool");
    fs::create_dir_all(&skill_dir).unwrap();
    symlink(&outside_skill, skill_dir.join(SKILL_FILE)).unwrap();
    write_manifest(root, "acme", "MIT", &[("tool/SKILL.md", &"f".repeat(64))]);

    let cat = discover_skills(root);
    assert!(cat.get("payload").is_none(), "наружный симлинк не загружен");
    let escape = cat
        .errors()
        .iter()
        .find(|(rel, _)| rel == "vendor/acme/tool/SKILL.md");
    assert_eq!(
        escape.map(|(_, e)| e),
        Some(&SkillError::PathEscape),
        "ожидался PathEscape (бэкстоп на vendor-уровне), errors={:?}",
        cat.errors()
    );
}

/// Симлинк-bundle-КАТАЛОГ наружу под vendor/ → не обходим (symlink_metadata не is_dir).
#[cfg(unix)]
#[test]
fn discover_vendored_symlinked_bundle_dir_skipped() {
    use std::os::unix::fs::symlink;
    let outside = TempDir::new().unwrap();
    // Снаружи — целая bundle-раскладка с валидным манифестом (чтобы доказать: дело в симлинке).
    let ob = outside.path().join("evilbundle");
    fs::create_dir_all(ob.join("tool")).unwrap();
    let content = "---\nname: evil\ndescription: outside\n---\nbody\n";
    fs::write(ob.join("tool").join(SKILL_FILE), content).unwrap();

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(VENDOR_DIR)).unwrap();
    symlink(&ob, root.join(VENDOR_DIR).join("evilbundle")).unwrap();

    let cat = discover_skills(root);
    assert!(
        cat.get("evil").is_none(),
        "симлинк-bundle-каталог наружу не обходится"
    );
}

/// vendor.lock с битым JSON → MissingManifest (не серде-yaml; парс-ошибка видима, не загружен).
#[test]
fn discover_vendored_malformed_manifest_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_vendored_skill(root, "acme", "tool", "vtool");
    fs::write(
        root.join(VENDOR_DIR).join("acme").join(VENDOR_LOCK_FILE),
        "{ this is not valid json",
    )
    .unwrap();

    let cat = discover_skills(root);
    assert!(cat.get("vtool").is_none());
    let err = cat
        .errors()
        .iter()
        .find(|(rel, _)| rel == "vendor/acme/tool/SKILL.md");
    assert!(
        matches!(err.map(|(_, e)| e), Some(SkillError::MissingManifest(_))),
        "битый JSON → MissingManifest, errors={:?}",
        cat.errors()
    );
}

/// vendor/<bundle>/references/ (подкаталог без SKILL.md) — пропущен, не кандидат.
#[test]
fn discover_vendored_references_dir_skipped() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let content = write_vendored_skill(root, "acme", "tool", "vtool");
    // Sibling references/ внутри bundle'а (не скилл).
    fs::create_dir_all(root.join(VENDOR_DIR).join("acme").join("references")).unwrap();
    fs::write(
        root.join(VENDOR_DIR)
            .join("acme")
            .join("references")
            .join("X.md"),
        "ref",
    )
    .unwrap();
    write_manifest(
        root,
        "acme",
        "MIT",
        &[("tool/SKILL.md", &sha256_of(&content))],
    );

    let cat = discover_skills(root);
    assert!(cat.errors().is_empty(), "{:?}", cat.errors());
    assert_eq!(cat.len(), 1, "references/ не стал скиллом");
}

/// REGRESSION: trusted-local скилл (без манифеста) грузится как и в SKILL-1 (vendoring не сломал).
#[test]
fn discover_trusted_local_still_loads_without_manifest() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_skill(root, "local", "local", "no manifest needed");
    let cat = discover_skills(root);
    assert_eq!(cat.len(), 1);
    let s = cat.get("local").unwrap();
    assert_eq!(s.tier, TrustTier::TrustedLocal);
    assert_eq!(s.license, None, "trusted-local не требует лицензии");
}

// ── SKILL-3: РЕАЛЬНЫЙ kepano-bundle на диске (offline, из репо) ──────────────────────────────────

/// Локализует `_skills/vendor/kepano` в корне репо относительно крейта (`CARGO_MANIFEST_DIR` =
/// crates/nexus-core → вверх на 2 = корень репо). `None`, если bundle не на месте (не валим тест,
/// но ассертим присутствие в самом тесте).
fn repo_kepano_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../_skills/vendor/kepano")
}

/// РЕАЛЬНЫЙ kepano-bundle: указываем discovery на skills_dir, СОДЕРЖАЩИЙ `vendor/kepano/...`, и
/// проверяем, что ОБА скилла грузятся ЧИСТО: license=="MIT", tier==Vendor, caps ⊆
/// {VaultRead,VaultWrite} (нет inert-заметки), hash-pin сошёлся. Полностью offline (файлы в репо).
#[test]
fn discover_real_kepano_bundle_loads_clean() {
    let kepano = repo_kepano_root();
    assert!(
        kepano.join(VENDOR_LOCK_FILE).is_file(),
        "ожидался реальный bundle на диске: {kepano:?}"
    );

    // skills_dir = временный корень, в котором `vendor/kepano` — симлинк/копия? Нет: discovery
    // ждёт `<skills_dir>/vendor/<bundle>/...`. Реальный bundle лежит как `_skills/vendor/kepano`,
    // значит skills_dir = `_skills`. Берём его и фильтруем по нашим двум именам (в каталоге могут
    // быть и другие top-level скиллы в будущем — мы проверяем именно kepano-пару).
    let skills_dir = kepano
        .parent() // .../vendor
        .and_then(|p| p.parent()) // .../_skills
        .expect("_skills root");
    let cat = discover_skills(skills_dir);

    for name in ["obsidian-markdown", "json-canvas"] {
        let s = cat
            .get(name)
            .unwrap_or_else(|| panic!("kepano-скилл `{name}` загружен, errors={:?}", cat.errors()));
        assert_eq!(s.tier, TrustTier::Vendor, "{name}: Vendor-tier");
        assert_eq!(
            s.license.as_deref(),
            Some("MIT"),
            "{name}: MIT из vendor.lock"
        );
        // caps ⊆ {VaultRead,VaultWrite}: реальные kepano-frontmatter без capabilities → пусто →
        // resolve даёт пустой inert (нет заявленных опасных способностей).
        let declared = parse_typed_capabilities(&s.capabilities);
        let res = resolve_capabilities(&declared, s.tier, &RunPolicy::phase_c_vault());
        assert!(
            !res.has_inert(),
            "{name}: нет inert-заметки (caps ⊆ vault): {:?}",
            res.inert
        );
    }
    // Никаких ошибок по kepano-скиллам (другие top-level записи нас не касаются).
    for (rel, e) in cat.errors() {
        assert!(
            !rel.starts_with("vendor/kepano/"),
            "kepano-скилл дал ошибку {rel}: {e:?}"
        );
    }
}
