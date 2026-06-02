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
    pub word_count: usize,
}

/// Разбирает markdown-документ.
pub fn parse(content: &str) -> ParsedDocument {
    let (frontmatter, body, fm_lines) = split_frontmatter(content);

    let analysis = analyze_with_pulldown(body, fm_lines);
    let (mut links, tags) = scan_wiki_and_tags(body, fm_lines, &analysis.code_ranges);
    links.extend(analysis.md_links);

    let title = frontmatter
        .and_then(frontmatter_title)
        .or(analysis.first_h1);

    ParsedDocument {
        title,
        frontmatter: frontmatter.map(str::to_owned),
        links,
        tags: tags.into_iter().collect(),
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
            if let Some(rel) = body[i + 2..].find("]]") {
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
            let mut j = start;
            while j < body.len() && is_tag_char(bytes[j]) {
                j += 1;
            }
            let tag = &body[start..j];
            // Тег должен содержать хотя бы одну букву (отсекает `#123` и заголовки `# H`).
            if tag.bytes().any(|c| c.is_ascii_alphabetic()) {
                tags.insert(tag.to_ascii_lowercase());
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

fn is_tag_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'/'
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
