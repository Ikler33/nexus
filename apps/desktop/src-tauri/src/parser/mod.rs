//! Markdown-парсер (§4.2): извлекает title, сырой frontmatter, исходящие ссылки
//! (`[[wiki]]`, `![[embed]]`, внутренние markdown-ссылки), `#tags` и счётчик слов.
//!
//! Структуру (заголовки, код-блоки, markdown-ссылки) даёт `pulldown-cmark`; `[[wikilinks]]`
//! и `#tags` (не-CommonMark) сканируются по сырому телу, НО матчи внутри кода исключаются
//! по диапазонам код-спанов/код-блоков из pulldown.

use std::collections::BTreeSet;
use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Тип исходящей ссылки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    Wikilink,
    Embed,
    Markdown,
}

impl LinkType {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkType::Wikilink => "wikilink",
            LinkType::Embed => "embed",
            LinkType::Markdown => "markdown",
        }
    }
}

/// Одна исходящая ссылка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLink {
    /// Цель без `#heading` и `|alias` (для wiki); для markdown — dest как есть.
    pub target_raw: String,
    pub link_type: LinkType,
    /// 1-based номер строки в исходном файле (с учётом frontmatter).
    pub line_number: usize,
    /// ~150 символов вокруг ссылки (для превью беклинков).
    pub context: String,
}

/// Результат разбора документа.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDocument {
    pub title: Option<String>,
    /// Сырой YAML-блок frontmatter (без ограничителей `---`), как есть.
    pub frontmatter: Option<String>,
    pub links: Vec<ParsedLink>,
    /// Нормализованные (lowercase) уникальные теги, отсортированы.
    pub tags: Vec<String>,
    /// Алиасы из frontmatter (`aliases:`/`alias:`) — для резолва `[[Алиас]]` (V4.1).
    pub aliases: Vec<String>,
    /// Плоские скалярные поля frontmatter верхнего уровня (`progress/due/goal/evergreen/draft`…) как
    /// `(ключ, значение)` — для кросс-файловых запросов (цели/stale-radar/Dataview). Списки/вложенный
    /// YAML сюда не попадают (см. [`frontmatter_fields`]); порядок — как в файле, ключи уникальны.
    pub fields: Vec<(String, String)>,
    pub word_count: usize,
}

/// Разбирает markdown-документ.
pub fn parse(content: &str) -> ParsedDocument {
    let (frontmatter, body, fm_lines) = split_frontmatter(content);

    let analysis = analyze_with_pulldown(body, fm_lines);
    let (mut links, mut tags) = scan_wiki_and_tags(body, fm_lines, &analysis.code_ranges);
    links.extend(analysis.md_links);
    // #35 хвост: frontmatter `tags:` — в общий набор (file_tags индексируется из parsed.tags).
    if let Some(fm) = frontmatter {
        tags.extend(frontmatter_tags(fm));
    }

    let title = frontmatter
        .and_then(frontmatter_title)
        .or(analysis.first_h1);

    ParsedDocument {
        title,
        frontmatter: frontmatter.map(str::to_owned),
        links,
        tags: tags.into_iter().collect(),
        aliases: frontmatter.map(frontmatter_aliases).unwrap_or_default(),
        fields: frontmatter.map(frontmatter_fields).unwrap_or_default(),
        word_count: body.split_whitespace().count(),
    }
}

struct Analysis {
    code_ranges: Vec<Range<usize>>,
    md_links: Vec<ParsedLink>,
    first_h1: Option<String>,
}

/// Один проход pulldown: диапазоны кода (для исключения), внутренние markdown-ссылки, первый H1.
fn analyze_with_pulldown(body: &str, fm_lines: usize) -> Analysis {
    let mut code_ranges = Vec::new();
    let mut md_links = Vec::new();
    let mut first_h1 = None;

    let mut code_block_start = None;
    let mut in_h1 = false;
    let mut h1_buf = String::new();

    for (event, range) in Parser::new_ext(body, Options::empty()).into_offset_iter() {
        match event {
            Event::Code(_) => code_ranges.push(range),
            Event::Start(Tag::CodeBlock(_)) => code_block_start = Some(range.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = code_block_start.take() {
                    code_ranges.push(start..range.end);
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                if is_internal_link(dest_url.as_ref()) {
                    md_links.push(ParsedLink {
                        target_raw: normalize_target(dest_url.as_ref())
                            .unwrap_or_else(|| dest_url.to_string()),
                        link_type: LinkType::Markdown,
                        line_number: fm_lines + count_newlines(body, range.start) + 1,
                        context: context_around(body, range.start),
                    });
                }
            }
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) if first_h1.is_none() => {
                in_h1 = true;
                h1_buf.clear();
            }
            Event::Text(t) if in_h1 => h1_buf.push_str(&t),
            Event::End(TagEnd::Heading(HeadingLevel::H1)) if in_h1 => {
                first_h1 = Some(h1_buf.trim().to_string());
                in_h1 = false;
            }
            _ => {}
        }
    }

    Analysis {
        code_ranges,
        md_links,
        first_h1,
    }
}

/// Сканирует сырое тело на `[[wiki]]` / `![[embed]]` и `#tags`, пропуская код-диапазоны.
fn scan_wiki_and_tags(
    body: &str,
    fm_lines: usize,
    code_ranges: &[Range<usize>],
) -> (Vec<ParsedLink>, BTreeSet<String>) {
    let in_code = |off: usize| code_ranges.iter().any(|r| r.contains(&off));
    let bytes = body.as_bytes();
    let mut links = Vec::new();
    let mut tags = BTreeSet::new();
    let mut i = 0;

    while i < body.len() {
        if body[i..].starts_with("[[") && !in_code(i) {
            // Закрывающие `]]` ищем ТОЛЬКО в текущей строке: несбалансированный `[[` без `]]` на своей
            // строке раньше матчил далёкие `]]` через абзацы → фантомные «многострочные» ссылки (аудит).
            // Obsidian-вики-ссылка всегда в пределах одной строки.
            let rest = &body[i + 2..];
            let line_end = rest.find('\n').unwrap_or(rest.len());
            if let Some(rel) = rest[..line_end].find("]]") {
                let inner = &body[i + 2..i + 2 + rel];
                let embed = i > 0 && bytes[i - 1] == b'!';
                if let Some(target) = normalize_target(inner) {
                    links.push(ParsedLink {
                        target_raw: target,
                        link_type: if embed {
                            LinkType::Embed
                        } else {
                            LinkType::Wikilink
                        },
                        line_number: fm_lines + count_newlines(body, i) + 1,
                        context: context_around(body, i),
                    });
                }
                i += 2 + rel + 2;
                continue;
            }
        }

        if bytes[i] == b'#' && !in_code(i) && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            let start = i + 1;
            // PROP-1: сканируем по СИМВОЛАМ (Unicode), а не байтам — иначе кириллица режется на
            // первом не-ASCII байте. Конец тега = первый недопустимый символ.
            let mut j = start;
            for (off, c) in body[start..].char_indices() {
                if is_tag_char(c) {
                    j = start + off + c.len_utf8();
                } else {
                    break;
                }
            }
            let tag = &body[start..j];
            // Тег должен содержать хотя бы одну букву (отсекает `#123` и заголовки `# H`).
            if tag.chars().any(char::is_alphabetic) {
                tags.insert(tag.to_lowercase());
                i = j;
                continue;
            }
        }

        i += utf8_len(bytes[i]);
    }

    (links, tags)
}

/// Отделяет YAML-frontmatter от тела: `(frontmatter, body, число строк до тела)`.
/// Переиспользуется чанкером (Ф1-2), чтобы тело шло в чанки без frontmatter.
pub(crate) fn split_frontmatter(content: &str) -> (Option<&str>, &str, usize) {
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return (None, content, 0);
    }
    let after_open = content.find('\n').map(|i| i + 1).unwrap_or(content.len());
    let rest = &content[after_open..];
    let mut idx = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            let fm = rest[..idx].trim_end_matches(['\n', '\r']);
            let body_start = after_open + idx + line.len();
            let body = content.get(body_start..).unwrap_or("");
            let line_offset = count_newlines(content, body_start);
            return (Some(fm), body, line_offset);
        }
        idx += line.len();
    }
    (None, content, 0)
}

fn frontmatter_title(fm: &str) -> Option<String> {
    fm.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("title:")?;
        let value = rest.trim().trim_matches(['"', '\'']).trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Извлекает алиасы из frontmatter (V4.1) минимальным line-парсером — без YAML-либы (serde_yaml
/// архивирован → security-гейт). Формы: `aliases: [A, B]` (инлайн), `alias: A` (скаляр) и блок:
/// ```text
/// aliases:
///   - A
///   - B
/// ```
/// Берёт ПЕРВЫЙ ключ `aliases:`/`alias:`. Сложный YAML (вложенность/многострочные) не покрывается —
/// полный typed-frontmatter — отдельным решением по YAML-подходу (NEEDS-DECISION).
fn frontmatter_aliases(fm: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut lines = fm.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed
            .strip_prefix("aliases:")
            .or_else(|| trimmed.strip_prefix("alias:"))
        else {
            continue;
        };
        let rest = rest.trim();
        if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            for item in inner.split(',') {
                push_alias(&mut out, item);
            }
        } else if rest.is_empty() {
            // Блочный список: подряд идущие `- value`.
            while let Some(next) = lines.peek() {
                match next.trim_start().strip_prefix('-') {
                    Some(item) => {
                        push_alias(&mut out, item);
                        lines.next();
                    }
                    None => break,
                }
            }
        } else {
            push_alias(&mut out, rest);
        }
        break; // только первый ключ aliases
    }
    out
}

/// Чистит значение алиаса (тримит кавычки/пробелы) и добавляет, если непусто и ещё нет.
fn push_alias(out: &mut Vec<String>, raw: &str) {
    let v = raw.trim().trim_matches(['"', '\'']).trim();
    if !v.is_empty() && !out.iter().any(|a| a == v) {
        out.push(v.to_string());
    }
}

/// Извлекает теги из frontmatter (`tags:`/`tag:`) тем же line-парсером, что и алиасы (#35 хвост:
/// раньше `tags: [goal]` НЕ давал file_tag — `scan_wiki_and_tags` сканирует только тело). Формы:
/// `tags: [a, b]` (инлайн), `tag: a` (скаляр), блочный список `- a`. Нормализация — как у
/// инлайн-тегов тела: срез ведущего `#` (форма `"#tag"` в кавычках), ASCII-lowercase, символы
/// [`is_tag_char`], хотя бы одна буква (отсекает `123`); иное значение пропускается.
fn frontmatter_tags(fm: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut lines = fm.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed
            .strip_prefix("tags:")
            .or_else(|| trimmed.strip_prefix("tag:"))
        else {
            continue;
        };
        let rest = rest.trim();
        if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            for item in inner.split(',') {
                push_tag(&mut out, item);
            }
        } else if rest.is_empty() {
            // Блочный список: подряд идущие `- value`.
            while let Some(next) = lines.peek() {
                match next.trim_start().strip_prefix('-') {
                    Some(item) => {
                        push_tag(&mut out, item);
                        lines.next();
                    }
                    None => break,
                }
            }
        } else {
            push_tag(&mut out, rest);
        }
        break; // только первый ключ tags
    }
    out
}

/// Нормализует значение тега frontmatter (см. [`frontmatter_tags`]) и добавляет, если валиден.
fn push_tag(out: &mut Vec<String>, raw: &str) {
    let v = raw.trim().trim_matches(['"', '\'']).trim();
    let v = v.strip_prefix('#').unwrap_or(v);
    // PROP-1: char-based (Unicode) — кириллические frontmatter-теги тоже валидны.
    if !v.is_empty() && v.chars().all(is_tag_char) && v.chars().any(char::is_alphabetic) {
        let v = v.to_lowercase();
        if !out.contains(&v) {
            out.push(v);
        }
    }
}

/// Извлекает ПЛОСКИЕ скалярные поля frontmatter верхнего уровня (typed-frontmatter) минимальным
/// line-парсером — без YAML-либы (serde_yaml архивирован → security-гейт; выбор владельца). Берёт
/// строки вида `ключ: значение` БЕЗ ведущих отступов (вложенное/блок-список — пропускаются), значение —
/// непустой скаляр (НЕ инлайн-список `[…]`/объект `{…}`; кавычки снимаются). Списки (`tags`/`aliases`)
/// и вложенный YAML сюда НЕ попадают — для них свои таблицы / сырой `frontmatter`. Ключи уникальны
/// (последний выигрывает), порядок — как в файле. Намеренно простой: PKM-frontmatter — плоские поля.
fn frontmatter_fields(fm: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in fm.lines() {
        // Только верхний уровень: без ведущих пробелов/таба (вложенность) и не элемент списка `-`.
        if line.starts_with([' ', '\t', '-']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        // Ключ — простой идентификатор (буквы/цифры/`_`/`-`); иначе это не «ключ: значение».
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
        {
            continue;
        }
        let value = read_scalar(value);
        // Только непустые скаляры: инлайн-список/объект и пустое (блок ниже) — не сюда.
        if value.is_empty() || value.starts_with('[') || value.starts_with('{') {
            continue;
        }
        if let Some(slot) = out.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value.to_string(); // последний ключ выигрывает
        } else {
            out.push((key.to_string(), value.to_string()));
        }
    }
    out
}

/// Ошибка записи frontmatter (BOARD-1): не перезаписываем вслепую (сохранность данных).
#[derive(Debug, PartialEq, Eq)]
pub enum FmWriteError {
    /// Открывающий `---` есть, но закрывающего нет — файл битый, не трогаем.
    Malformed,
    /// Значение нельзя записать так, чтобы [`read_scalar`] прочитал его ОБРАТНО тем же (перевод строки,
    /// краевые кавычки, инлайн-список) — читатель — тупой edge-stripper, не YAML-парсер. Лучше явная
    /// ошибка, чем тихая порча: `say "hi"` потеряло бы хвостовую кавычку при чтении.
    Unrepresentable,
    /// m8: целевой ключ существует, но его ТЕКУЩЕЕ значение — НЕ плоский скаляр (инлайн-список `[…]`,
    /// инлайн-объект `{…}` или блок-список из `- ` ниже). Перезаписать его как скаляр осиротило бы
    /// `- a`/`- b` строки или потеряло бы инлайн-список (читатель такие ключи не видит как скаляр —
    /// см. [`frontmatter_fields`]). Round-trip-reject: лучше явная ошибка, чем тихая порча. Файл не трогаем.
    NonScalarTarget,
}

/// ЕДИНСТВЕННАЯ точка очистки скалярного значения frontmatter — общий источник для чтения
/// ([`frontmatter_fields`]) и для проверки round-trip при записи ([`fm_value_repr`]), чтобы они НИКОГДА
/// не разошлись. Тупой edge-stripper: снимает краевые пробелы, затем краевые `"`/`'`, затем снова пробелы.
fn read_scalar(raw: &str) -> &str {
    raw.trim().trim_matches(['"', '\'']).trim()
}

/// Кодирует значение для записи во frontmatter, ЕСЛИ оно переживёт round-trip через [`read_scalar`].
/// Возвращает `None`, если читатель прочитал бы НЕ то же самое (→ `Err(Unrepresentable)` вместо порчи):
/// перевод строки (сломает структуру блока), краевые кавычки/escape (edge-stripper их съест), пустое
/// или `[`/`{` (читатель пропустит как инлайн-список/объект). Нормальные значения kanban
/// (`todo`, даты, `project: Имя`, даже `a: b` с двоеточием) проходят.
fn fm_value_repr(value: &str) -> Option<String> {
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return None;
    }
    let quoted = quote_yaml_value(value);
    let read = read_scalar(&quoted);
    if read != value || read.starts_with('[') || read.starts_with('{') {
        return None;
    }
    Some(quoted)
}

/// Та же логика матчинга ключа, что у [`frontmatter_fields`] (точная копия — НЕ regex `^key:`):
/// верхний уровень (без ведущих пробелов/таба/`-`), `split_once(':')`, `key.trim()==target`, ключ —
/// идентификатор (буквы/цифры/`_`/`-`).
fn is_field_line(line: &str, target: &str) -> bool {
    if line.starts_with([' ', '\t', '-']) {
        return false;
    }
    let Some((k, _)) = line.split_once(':') else {
        return false;
    };
    let k = k.trim();
    !k.is_empty()
        && k == target
        && k.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
}

/// Голый YAML block-scalar-индикатор: `|`/`>` с опц. chomp (`+`/`-`) и indent-цифрой (`|`, `>`, `|-`,
/// `>2`, `|2-`…). Такое «значение» означает, что НИЖЕ идёт многострочный блок-литерал — перезапись
/// его скаляром осиротила бы продолжающие строки. (Читатель показывает такой ключ со значением «|» —
/// это его собственный баг; писатель обязан НЕ затирать, иначе тихая порча, см. m8 review.)
fn is_block_scalar_indicator(value: &str) -> bool {
    let mut ch = value.chars();
    matches!(ch.next(), Some('|' | '>')) && ch.all(|c| matches!(c, '+' | '-' | '0'..='9'))
}

/// m8: текущее значение совпавшего ключа — НЕ плоский скаляр (значит, перезапись как скаляра осиротила
/// бы/потеряла бы вложенный блок)? Симметрично читателю [`frontmatter_fields`] (через тот же
/// [`read_scalar`]) и блок-парсерам [`frontmatter_aliases`]/[`frontmatter_tags`] — НЕ второй парсер.
/// `key_line` — строка `ключ: значение` без EOL; `next_line` — следующая строка региона frontmatter
/// без EOL (если есть). Не-скаляр, если значение после [`read_scalar`]:
///   • инлайн-список/объект (`[`/`{`) — читатель пропускает как нескаляр; перезапись потеряла бы список;
///   • ПУСТОЕ или голый block-scalar-индикатор (`|`/`>`) И следующая строка — отступная продолжающая
///     (дочерний маппинг `  sub:` / литерал `  l1`) ИЛИ элемент списка `- …`: ключ владеет блоком ниже,
///     перезапись скаляром осиротила бы его строки.
/// Пустое значение БЕЗ блока ниже (просто `key:` + не-отступная строка/конец) — НЕ нескаляр: безопасно
/// заполнить. ОГРАНИЧЕНИЕ: блок, отделённый ПУСТОЙ строкой (`key:\n\n  - a`), не ловим — симметрично
/// читателю, чьи блок-парсеры рвутся на пустой строке (см. BACKLOG m8-хвост для произвольных ключей).
fn is_non_scalar_target(key_line: &str, next_line: Option<&str>) -> bool {
    let Some((_, value)) = key_line.split_once(':') else {
        return false;
    };
    let value = read_scalar(value);
    if value.starts_with('[') || value.starts_with('{') {
        return true; // инлайн-список/объект
    }
    if value.is_empty() || is_block_scalar_indicator(value) {
        // Ключ владеет блоком ниже, если следующая строка — отступная продолжающая (дочерний
        // маппинг/литерал) ИЛИ элемент списка `- …` (peek как у frontmatter_aliases/tags).
        if let Some(next) = next_line {
            let trimmed = next.trim_start();
            return next.len() != trimmed.len() || trimmed.starts_with('-');
        }
    }
    false
}

/// Нужно ли квотировать скаляр-значение YAML (BOARD-1, защита от потери данных). Покрывает
/// YAML-индикаторы в начале, блок-конструкции `- `/`? `/`: `, флоу-символы, висячий `:`, комментарий
/// ` #`, резерв-слова (null/bool во всех регистрах), пустое/с краевыми пробелами. Симметрично чтению
/// [`frontmatter_fields`], которое БЕЗУСЛОВНО снимает кавычки (→ round-trip).
fn needs_yaml_quote(v: &str) -> bool {
    if v.is_empty() || v != v.trim() {
        return true;
    }
    let first = v.chars().next().unwrap();
    if "!&*?|>%@`\"'#,[]{}".contains(first) {
        return true;
    }
    if (first == '-' || first == '?' || first == ':') && (v.len() == 1 || v[1..].starts_with(' ')) {
        return true;
    }
    if v.ends_with(':') || v.contains(" #") || v.contains(": ") {
        return true;
    }
    matches!(
        v.to_ascii_lowercase().as_str(),
        "null" | "~" | "true" | "false" | "yes" | "no" | "on" | "off"
    )
}

/// Квотирует значение для записи во frontmatter, если нужно (двойные кавычки, экранирование `\`/`"`).
fn quote_yaml_value(v: &str) -> String {
    if needs_yaml_quote(v) {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        v.to_string()
    }
}

/// BOARD-1: правит/добавляет ОДИН плоский top-level frontmatter-ключ, сохраняя остальной YAML и тело
/// байт-в-байт. serde_yaml архивирован → ручной write-back; это единая точка записи frontmatter (статус
/// при DnD, project/priority/due, Properties-панель). Нет frontmatter-блока — создаётся. Незакрытый блок
/// (`---` без пары) → `Err(Malformed)`; значение, которое не переживёт round-trip через читатель
/// (перевод строки/краевые кавычки/инлайн-список) → `Err(Unrepresentable)`; целевой ключ уже хранит
/// СПИСОК/блок-родитель (инлайн `[…]`/`{…}` или блок `- ` ниже) → `Err(NonScalarTarget)` (m8: иначе
/// осиротили бы `- a`/`- b` или потеряли инлайн-список) — файл во всех случаях НЕ трогаем. Дубль-ключ:
/// правим ПОСЛЕДНЕЕ вхождение (читатель — last-key-wins).
pub fn set_frontmatter_field(
    content: &str,
    key: &str,
    value: &str,
) -> Result<String, FmWriteError> {
    let quoted = fm_value_repr(value).ok_or(FmWriteError::Unrepresentable)?;

    // Нет frontmatter-блока — создаём, тело сохраняем как есть.
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return Ok(format!("---\n{key}: {quoted}\n---\n\n{content}"));
    }

    let open_end = content.find('\n').map(|i| i + 1).unwrap_or(content.len());
    let rest = &content[open_end..];
    // Ищем закрывающий `---` (как split_frontmatter).
    let mut close_rel = None;
    let mut idx = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            close_rel = Some(idx);
            break;
        }
        idx += line.len();
    }
    let Some(close_rel) = close_rel else {
        return Err(FmWriteError::Malformed);
    };
    let fm_region = &rest[..close_rel];
    let close_and_body = &rest[close_rel..];

    // Строки региона с сохранением окончаний; правим ПОСЛЕДНЕЕ совпадение (читатель: last-key-wins).
    let lines: Vec<&str> = fm_region.split_inclusive('\n').collect();
    let last_match = lines
        .iter()
        .rposition(|l| is_field_line(l.trim_end_matches(['\n', '\r']), key));

    let mut new_fm = String::new();
    match last_match {
        Some(mi) => {
            // m8: нельзя перезаписать как скаляр ключ, чьё текущее значение — список/блок-родитель
            // (осиротит `- a`/`- b` или потеряет инлайн-список). Round-trip-reject, файл не трогаем.
            let matched = lines[mi].trim_end_matches(['\n', '\r']);
            let next = lines.get(mi + 1).map(|l| l.trim_end_matches(['\n', '\r']));
            if is_non_scalar_target(matched, next) {
                return Err(FmWriteError::NonScalarTarget);
            }
            for (i, line) in lines.iter().enumerate() {
                if i == mi {
                    let trimmed = line.trim_end_matches(['\n', '\r']);
                    let colon = trimmed.find(':').unwrap();
                    let ending = &line[trimmed.len()..]; // "\n" / "\r\n" / ""
                    new_fm.push_str(&trimmed[..colon]);
                    new_fm.push_str(": ");
                    new_fm.push_str(&quoted);
                    new_fm.push_str(ending);
                } else {
                    new_fm.push_str(line);
                }
            }
        }
        None => {
            new_fm.push_str(fm_region);
            if !new_fm.is_empty() && !new_fm.ends_with('\n') {
                new_fm.push('\n');
            }
            // EOL новой строки — как у блока (CRLF, если открывающий `---` был CRLF), без mixed-EOL.
            let eol = if content.starts_with("---\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            new_fm.push_str(key);
            new_fm.push_str(": ");
            new_fm.push_str(&quoted);
            new_fm.push_str(eol);
        }
    }

    Ok(format!(
        "{}{}{}",
        &content[..open_end],
        new_fm,
        close_and_body
    ))
}

/// Нормализует цель ссылки: убирает `|alias` и `#heading`, тримит. `None`, если пусто.
fn normalize_target(raw: &str) -> Option<String> {
    let no_alias = raw.split('|').next().unwrap_or(raw);
    let no_heading = no_alias.split('#').next().unwrap_or(no_alias);
    let t = no_heading.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn is_internal_link(dest: &str) -> bool {
    !dest.is_empty()
        && !dest.starts_with('#')
        && !dest.starts_with("//")
        && !dest.contains("://")
        && !dest.starts_with("mailto:")
        && !dest.starts_with("tel:")
}

/// Допустимый символ тега. PROP-1: Unicode-буквы/цифры (кириллица и пр.), а не только ASCII —
/// `#тег` для русскоязычного vault теперь валиден (Obsidian допускает Unicode-теги).
fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

fn count_newlines(s: &str, upto: usize) -> usize {
    s.as_bytes()[..upto.min(s.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

fn utf8_len(lead: u8) -> usize {
    match lead {
        b if b < 0x80 => 1,
        b if b >> 5 == 0b110 => 2,
        b if b >> 4 == 0b1110 => 3,
        _ => 4,
    }
}

fn context_around(body: &str, off: usize) -> String {
    let start = body[..off.min(body.len())]
        .char_indices()
        .rev()
        .take(50)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end = body[off.min(body.len())..]
        .char_indices()
        .take(120)
        .last()
        .map(|(i, c)| off + i + c.len_utf8())
        .unwrap_or(body.len());
    body[start..end]
        .replace(['\n', '\r'], " ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_frontmatter_title_links_tags() {
        let doc = parse(
            "---\ntitle: My Note\naliases: [Alt]\n---\n# Heading\n\nSee [[Other Note]] and \
             [[Target#Section|Alias]]. Embed ![[Diagram.png]].\nTags: #project #area/sub\n",
        );
        assert_eq!(doc.title.as_deref(), Some("My Note")); // frontmatter имеет приоритет над H1
        assert_eq!(
            doc.frontmatter.as_deref(),
            Some("title: My Note\naliases: [Alt]")
        );

        let wl: Vec<_> = doc
            .links
            .iter()
            .map(|l| (l.target_raw.as_str(), l.link_type))
            .collect();
        assert!(wl.contains(&("Other Note", LinkType::Wikilink)));
        assert!(wl.contains(&("Target", LinkType::Wikilink))); // #Section|Alias срезаны
        assert!(wl.contains(&("Diagram.png", LinkType::Embed)));

        assert_eq!(
            doc.tags,
            vec!["area/sub".to_string(), "project".to_string()]
        );
        assert_eq!(doc.aliases, vec!["Alt".to_string()]);
    }

    /// Аудит: несбалансированный `[[` без `]]` на своей строке НЕ матчит далёкие `]]` через абзацы
    /// (раньше плодил фантомную «многострочную» ссылку). Сбалансированная на строке — парсится.
    #[test]
    fn unbalanced_wikilink_does_not_match_across_lines() {
        let doc = parse("[[Unclosed link here\n\nдалёкий текст ]] потом\n");
        assert!(
            doc.links.is_empty(),
            "несбалансированный [[ не создаёт ссылку через строки"
        );
        let doc2 = parse("есть [[Valid]] ссылка на строке\n");
        assert!(doc2.links.iter().any(|l| l.target_raw == "Valid"));
    }

    #[test]
    fn frontmatter_aliases_inline_block_scalar() {
        // инлайн-список (с кавычками и пробелами)
        assert_eq!(
            parse("---\naliases: [Alt, \"Second Name\", 'Third']\n---\nbody\n").aliases,
            vec![
                "Alt".to_string(),
                "Second Name".to_string(),
                "Third".to_string()
            ]
        );
        // блочный список (прерывается на следующем ключе)
        assert_eq!(
            parse("---\ntitle: X\naliases:\n  - One\n  - Two\ntags: [t]\n---\nbody\n").aliases,
            vec!["One".to_string(), "Two".to_string()]
        );
        // скаляр (alias: единственное число тоже)
        assert_eq!(
            parse("---\nalias: Solo\n---\nb\n").aliases,
            vec!["Solo".to_string()]
        );
        // нет алиасов / нет frontmatter
        assert!(parse("---\ntitle: X\n---\nb\n").aliases.is_empty());
        assert!(parse("no frontmatter [[X]]\n").aliases.is_empty());
    }

    /// #35 хвост: frontmatter `tags:` попадают в `parsed.tags` (→ file_tags при индексации),
    /// с нормализацией инлайн-тегов тела (lowercase, ASCII-набор, срез `#`).
    #[test]
    fn frontmatter_tags_inline_block_scalar_merge_with_body() {
        // Инлайн-список: кавычки/`#`/регистр нормализуются; `123` (без буквы) — мимо; кириллица — ОК (PROP-1).
        assert_eq!(
            parse("---\ntags: [Goal, \"#project\", 123, 'тег']\n---\nbody\n").tags,
            vec!["goal".to_string(), "project".to_string(), "тег".to_string()]
        );
        // Блочный список + слияние с тегами тела (дедуп, сортировка BTreeSet).
        assert_eq!(
            parse("---\ntags:\n  - beta\n  - alpha\n---\nbody #alpha #zeta\n").tags,
            vec!["alpha".to_string(), "beta".to_string(), "zeta".to_string()]
        );
        // Скаляр `tag:`; без frontmatter — только теги тела.
        assert_eq!(
            parse("---\ntag: solo\n---\nb\n").tags,
            vec!["solo".to_string()]
        );
        assert_eq!(parse("b #only\n").tags, vec!["only".to_string()]);
    }

    /// PROP-1: Unicode/кириллица-теги — инлайн `#тег`, регистр, вложенность, frontmatter, ASCII не сломан.
    #[test]
    fn unicode_cyrillic_tags() {
        // Инлайн кириллица + lowercase + вложенный; граница на пробеле/пунктуации.
        assert_eq!(
            parse("текст #Идея и #проект/важное, конец\n").tags,
            vec!["идея".to_string(), "проект/важное".to_string()]
        );
        // `#123` без буквы — мимо; смешанный латиница+кириллица — ок.
        assert_eq!(parse("#123 #v2тест\n").tags, vec!["v2тест".to_string()]);
        // Frontmatter-список с кириллицей.
        assert_eq!(
            parse("---\ntags: [Проект, идея]\n---\nтело\n").tags,
            vec!["идея".to_string(), "проект".to_string()]
        );
        // ASCII-путь не сломан.
        assert_eq!(
            parse("body #alpha #Beta-1\n").tags,
            vec!["alpha".to_string(), "beta-1".to_string()]
        );
    }

    /// BOARD-1: write-back одного ключа — замена/добавление/создание, сохранность, квотирование, round-trip.
    #[test]
    fn set_frontmatter_field_write_back() {
        // Замена существующего ключа — остальное байт-в-байт (другой ключ + тело целы).
        let src = "---\nstatus: todo\nproject: Alpha\n---\n# H\nтело\n";
        let out = set_frontmatter_field(src, "status", "doing").unwrap();
        assert_eq!(out, "---\nstatus: doing\nproject: Alpha\n---\n# H\nтело\n");
        assert_eq!(
            parse(&out).fields,
            parse("---\nstatus: doing\nproject: Alpha\n---\n").fields
        );

        // Добавление отсутствующего ключа — перед закрывающим ---.
        let out = set_frontmatter_field(src, "priority", "high").unwrap();
        assert_eq!(
            out,
            "---\nstatus: todo\nproject: Alpha\npriority: high\n---\n# H\nтело\n"
        );

        // Нет frontmatter — создаётся, тело сохранено.
        let out = set_frontmatter_field("просто тело\n", "status", "todo").unwrap();
        assert_eq!(out, "---\nstatus: todo\n---\n\nпросто тело\n");

        // Незакрытый frontmatter — Err, файл не трогаем.
        assert_eq!(
            set_frontmatter_field("---\nstatus: todo\nбез закрытия\n", "status", "x"),
            Err(FmWriteError::Malformed)
        );

        // CRLF сохраняется при ЗАМЕНЕ.
        let out = set_frontmatter_field("---\r\nstatus: todo\r\n---\r\nbody\r\n", "status", "done")
            .unwrap();
        assert_eq!(
            out,
            "---\r\nstatus: todo\r\n---\r\nbody\r\n".replace("todo", "done")
        );

        // F5: ДОБАВЛЕНИЕ ключа в CRLF-файл — новая строка тоже CRLF (без mixed-EOL).
        let out =
            set_frontmatter_field("---\r\nstatus: todo\r\n---\r\nbody\r\n", "priority", "high")
                .unwrap();
        assert_eq!(
            out,
            "---\r\nstatus: todo\r\npriority: high\r\n---\r\nbody\r\n"
        );

        // F4: дубль-ключ — правим ПОСЛЕДНЕЕ вхождение (читатель: last-key-wins), первое не трогаем.
        let dup = "---\nstatus: a\nstatus: b\n---\nbody\n";
        let out = set_frontmatter_field(dup, "status", "c").unwrap();
        assert_eq!(out, "---\nstatus: a\nstatus: c\n---\nbody\n");
        // Читатель действительно видит новое значение.
        assert_eq!(
            parse(&out).fields,
            vec![("status".to_string(), "c".to_string())]
        );
    }

    /// BOARD-1 (adversarial F1/F2): значение, которое читатель НЕ прочитал бы обратно тем же
    /// (краевые кавычки / перевод строки / инлайн-список), → `Err(Unrepresentable)`, файл не трогаем.
    #[test]
    fn set_frontmatter_field_rejects_unrepresentable() {
        let base = "---\nx: old\n---\nbody\n";
        for bad in [
            "say \"hi\"",   // хвостовая кавычка — edge-stripper её съест
            "'urgent'",     // обёрнут кавычками целиком
            "a: \"b\"",     // нужно квотировать + содержит кавычку (escape необратим)
            "line1\nline2", // перевод строки — сломал бы структуру блока
            "tail\r",       // CR
            "[a, b]",       // инлайн-список — читатель пропустил бы
            "{a: 1}",       // инлайн-объект
            "",             // пустое — читатель пропустил бы
        ] {
            assert_eq!(
                set_frontmatter_field(base, "status", bad),
                Err(FmWriteError::Unrepresentable),
                "должно отвергнуть «{bad}»"
            );
        }
        // Интерьерные кавычки (НЕ на краю) — допустимы, round-trip целый.
        let out = set_frontmatter_field(base, "status", "say \"hi\" there").unwrap();
        assert_eq!(
            parse(&out).fields,
            vec![
                ("x".to_string(), "old".to_string()),
                ("status".to_string(), "say \"hi\" there".to_string()),
            ]
        );
    }

    /// m8: целевой ключ хранит СПИСОК/блок-родитель → `Err(NonScalarTarget)`, файл байт-в-байт ЦЕЛ
    /// (никаких осиротевших `- a`/`- b`, потери инлайн-списка или частичной правки). Симметрично читателю:
    /// такие ключи `frontmatter_fields` не видит как скаляр, значит писать в них как в скаляр нельзя.
    #[test]
    fn set_frontmatter_field_refuses_non_scalar_target() {
        // (b) Инлайн-список — перезапись потеряла бы `[a, b]`; файл не трогаем.
        let inline = "---\nstatus: [a, b]\nproject: Alpha\n---\n# H\nтело\n";
        assert_eq!(
            set_frontmatter_field(inline, "status", "doing"),
            Err(FmWriteError::NonScalarTarget)
        );
        assert_eq!(
            set_frontmatter_field(inline, "status", "doing").err(),
            Some(FmWriteError::NonScalarTarget)
        );

        // (c) Блок-список (`- ` ниже) — перезапись осиротила бы `- a`/`- b`; файл не трогаем.
        let block = "---\ntags:\n  - a\n  - b\nproject: Alpha\n---\nbody\n";
        assert_eq!(
            set_frontmatter_field(block, "tags", "x"),
            Err(FmWriteError::NonScalarTarget)
        );

        // Инлайн-объект `{…}` — тоже нескаляр.
        let obj = "---\nmeta: {a: 1}\n---\nbody\n";
        assert_eq!(
            set_frontmatter_field(obj, "meta", "v"),
            Err(FmWriteError::NonScalarTarget)
        );

        // (f) Запись СКАЛЯРА в ключ НАД чужим блок-списком не съедает чужой список.
        let above = "---\nstatus: todo\ntags:\n  - a\n  - b\n---\nbody\n";
        let out = set_frontmatter_field(above, "status", "doing").unwrap();
        assert_eq!(out, "---\nstatus: doing\ntags:\n  - a\n  - b\n---\nbody\n");
        // Читатель: status — обновлённый скаляр, блок-список tags ЦЕЛ.
        assert_eq!(
            parse(&out).fields,
            vec![("status".to_string(), "doing".to_string())]
        );
        assert_eq!(parse(&out).tags, vec!["a".to_string(), "b".to_string()]);

        // Дубль-ключ: чинится ПОСЛЕДНЕЕ вхождение — если оно список, отказываем (а не правим первое).
        let dup_block = "---\ntags: scalar\ntags:\n  - a\n  - b\n---\nbody\n";
        assert_eq!(
            set_frontmatter_field(dup_block, "tags", "x"),
            Err(FmWriteError::NonScalarTarget)
        );

        // Пустое значение БЕЗ блок-списка ниже (`key:` затем другой ключ) — НЕ нескаляр: заполняем.
        let empty = "---\nstatus:\nproject: Alpha\n---\nbody\n";
        let out = set_frontmatter_field(empty, "status", "doing").unwrap();
        assert_eq!(out, "---\nstatus: doing\nproject: Alpha\n---\nbody\n");

        // Пустое значение как ПОСЛЕДНЯЯ строка региона (нет следующей строки) — НЕ нескаляр: заполняем.
        let empty_last = "---\nproject: Alpha\nstatus:\n---\nbody\n";
        let out = set_frontmatter_field(empty_last, "status", "doing").unwrap();
        assert_eq!(out, "---\nproject: Alpha\nstatus: doing\n---\nbody\n");

        // (m8-review HIGH) Блок-СКАЛЯР `|`/`>` (литерал/folded ± chomp) — перезапись осиротила бы строки
        // блока (читатель ошибочно показывает значение «|», но писать туда скаляр = тихая порча).
        let block_scalar = "---\ndesc: |\n  l1\n  l2\nproject: A\n---\nbody\n";
        assert_eq!(
            set_frontmatter_field(block_scalar, "desc", "v"),
            Err(FmWriteError::NonScalarTarget)
        );
        let folded = "---\ndesc: >-\n  l1\n---\nbody\n";
        assert_eq!(
            set_frontmatter_field(folded, "desc", "v"),
            Err(FmWriteError::NonScalarTarget)
        );

        // (m8-review HIGH) Вложенный блок-МАППИНГ — дочерние `  sub:` отступные; перезапись осиротила
        // бы их (читатель исключает такой ключ из fields, см. nested-тест).
        let nested = "---\nnested:\n  sub: 1\nproject: Alpha\n---\nbody\n";
        assert_eq!(
            set_frontmatter_field(nested, "nested", "v"),
            Err(FmWriteError::NonScalarTarget)
        );

        // Не-FP: вырожденный `|` БЕЗ отступного блока ниже (следующая строка — другой ключ) — заполняем.
        let degenerate = "---\ndesc: |\nproject: A\n---\nbody\n";
        let out = set_frontmatter_field(degenerate, "desc", "v").unwrap();
        assert_eq!(out, "---\ndesc: v\nproject: A\n---\nbody\n");
    }

    /// BOARD-1: квотирование спецсимволов + round-trip через чтение frontmatter_fields (снимает кавычки).
    #[test]
    fn set_frontmatter_field_quoting_round_trip() {
        // (Краевые пробелы НЕ тестируем round-trip: reader frontmatter_fields делает финальный .trim()
        //  после снятия кавычек → нормализует пробелы; для status/project это несущественно.)
        for val in [
            "a: b",       // двоеточие-пробел
            "true",       // резерв-слово
            "#hash",      // комментарий-индикатор
            "- dash",     // блок-список
            "value # c",  // комментарий
            "обычный",    // без квот
            "2025-03-23", // дата — без квот (читается строкой)
        ] {
            let out = set_frontmatter_field("---\nx: old\n---\nbody\n", "status", val).unwrap();
            // Идемпотентность: parse∘write∘parse == значение (кавычки сняты чтением).
            let got: Vec<_> = parse(&out)
                .fields
                .into_iter()
                .filter(|(k, _)| k == "status")
                .collect();
            assert_eq!(
                got,
                vec![("status".to_string(), val.to_string())],
                "round-trip «{val}»"
            );
            // Повторная запись того же — стабильна (write∘parse-значения == write).
            let out2 = set_frontmatter_field(&out, "status", val).unwrap();
            assert_eq!(out, out2, "идемпотентность записи «{val}»");
        }
    }

    #[test]
    fn frontmatter_fields_extracts_flat_scalars_only() {
        let doc = parse(
            "---\n\
             title: My Note\n\
             progress: 0.5\n\
             due: 2026-01-01\n\
             goal: \"Ship v1\"\n\
             evergreen: true\n\
             aliases: [A, B]\n\
             tags: [x, y]\n\
             nested:\n  sub: 1\n\
             list:\n  - a\n  - b\n\
             ---\nbody\n",
        );
        // Плоские скаляры попадают (кавычки сняты), порядок как в файле.
        assert_eq!(
            doc.fields,
            vec![
                ("title".to_string(), "My Note".to_string()),
                ("progress".to_string(), "0.5".to_string()),
                ("due".to_string(), "2026-01-01".to_string()),
                ("goal".to_string(), "Ship v1".to_string()),
                ("evergreen".to_string(), "true".to_string()),
            ],
            "инлайн-списки (aliases/tags), вложенное (nested) и блок-списки (list) исключены"
        );
    }

    #[test]
    fn frontmatter_fields_last_key_wins_and_empty_cases() {
        // дубль ключа → последний выигрывает
        assert_eq!(
            parse("---\nstatus: draft\nstatus: final\n---\nb\n").fields,
            vec![("status".to_string(), "final".to_string())]
        );
        // нет frontmatter / только списки → пусто
        assert!(parse("body only\n").fields.is_empty());
        assert!(parse("---\naliases: [A]\n---\nb\n").fields.is_empty());
    }

    #[test]
    fn title_falls_back_to_first_h1() {
        let doc = parse("# Title From Heading\n\nbody [[X]]\n");
        assert_eq!(doc.title.as_deref(), Some("Title From Heading"));
        assert_eq!(doc.links.len(), 1);
    }

    #[test]
    fn ignores_wikilinks_and_tags_inside_code() {
        let doc = parse(
            "Real [[Link]] #real\n\n```\n[[NotALink]] #nottag\n```\n\nInline `[[Nope]] #nope`.\n",
        );
        let targets: Vec<_> = doc.links.iter().map(|l| l.target_raw.as_str()).collect();
        assert_eq!(targets, vec!["Link"]); // из кода — исключены
        assert_eq!(doc.tags, vec!["real".to_string()]);
    }

    #[test]
    fn captures_internal_markdown_links_only() {
        let doc = parse("[internal](Notes/Other.md) and [web](https://example.com).\n");
        let internal: Vec<_> = doc
            .links
            .iter()
            .filter(|l| l.link_type == LinkType::Markdown)
            .map(|l| l.target_raw.as_str())
            .collect();
        assert_eq!(internal, vec!["Notes/Other.md"]); // внешний http исключён
    }

    #[test]
    fn line_numbers_account_for_frontmatter() {
        let doc = parse("---\ntitle: T\n---\nline one\n[[Link]] on line two\n");
        let link = doc.links.iter().find(|l| l.target_raw == "Link").unwrap();
        assert_eq!(link.line_number, 5); // ---(1) title(2) ---(3) line one(4) link(5)
    }
}
