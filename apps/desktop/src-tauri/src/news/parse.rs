//! Парсинг фидов в [`NewsEntry`] (NF-1, AC-NF-1): RSS 2.0 и Atom — через `quick-xml`
//! (низкоуровневый токенизатор: CDATA/энтити/неймспейсы руками ненадёжно), нормализация и
//! выжимка — свои; HF daily_papers и HN Algolia — JSON через serde. Даты — ручные RFC 3339 /
//! RFC 2822 без chrono (реюз `days_from_civil`, прецедент `home::stale`). Контент фидов
//! НЕДОВЕРЕННЫЙ — здесь он только нормализуется в plain-text, никогда не интерпретируется.

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;

use super::{FeedKind, NewsEntry, NewsError, EXCERPT_MAX_CHARS};
use crate::home::stale::days_from_civil;

/// Разбирает тело фида в нормализованные записи. Битый вход → `Err` (источник пропускается
/// прогоном с видимой ошибкой, AC-NF-1); записи без url/title отбрасываются молча-внутри
/// (это мусорные элементы фида, не сбой источника).
pub fn parse_feed(
    kind: FeedKind,
    source_id: &str,
    body: &str,
) -> Result<Vec<NewsEntry>, NewsError> {
    match kind {
        FeedKind::Rss => parse_xml(source_id, body, XmlDialect::Rss),
        FeedKind::Atom => parse_xml(source_id, body, XmlDialect::Atom),
        FeedKind::HfDailyPapers => parse_hf(source_id, body),
        FeedKind::HnAlgolia => parse_hn(source_id, body),
    }
}

// ── XML (RSS 2.0 / Atom) ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum XmlDialect {
    Rss,
    Atom,
}

/// Один проход pull-парсером: внутри `<item>`/`<entry>` копим текст интересующих тегов;
/// Atom-ссылка — атрибут `href` (`rel="alternate"` или без rel).
fn parse_xml(
    source_id: &str,
    body: &str,
    dialect: XmlDialect,
) -> Result<Vec<NewsEntry>, NewsError> {
    let item_tag: &[u8] = match dialect {
        XmlDialect::Rss => b"item",
        XmlDialect::Atom => b"entry",
    };
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(false);
    // Голый `&` (без `;`) в тексте — норма для неряшливых RSS (URL с `?x=1&y=2`, «News & Views»
    // на уровне канала). quick-xml ≥0.38 валидирует ссылки-на-сущности во ВСЕХ text-узлах уже на
    // read_event (0.37 сканил только при unescape() захватываемых полей item'а) — без флага такой фид
    // умирал бы ЦЕЛИКОМ (IllFormed(UnclosedReference)). С флагом голый `&` приходит обычным
    // Text-событием с литеральным `&`. Осознанное улучшение availability vs 0.37: до миграции
    // сырой `&` в захватываемом поле тоже ронял фид (см. пин-тест + CHANGELOG).
    reader.config_mut().allow_dangling_amp = true;

    let mut out = Vec::new();
    let mut in_item = false;
    let mut field: Option<Field> = None;
    let mut title = String::new();
    let mut link = String::new();
    let mut date = String::new();
    let mut desc = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if name == item_tag {
                    in_item = true;
                    (title, link, date, desc) = Default::default();
                } else if in_item {
                    field = match (dialect, name) {
                        (_, b"title") => Some(Field::Title),
                        (XmlDialect::Rss, b"link") => Some(Field::Link),
                        (XmlDialect::Rss, b"pubDate") | (XmlDialect::Atom, b"published") => {
                            Some(Field::Date)
                        }
                        // Atom: `updated` — фолбэк, если `published` не встречался.
                        (XmlDialect::Atom, b"updated") if date.is_empty() => Some(Field::Date),
                        (XmlDialect::Rss, b"description") | (XmlDialect::Atom, b"summary") => {
                            Some(Field::Desc)
                        }
                        // Atom-`content` — фолбэк при пустом summary (релизы GitHub).
                        (XmlDialect::Atom, b"content") if desc.is_empty() => Some(Field::Desc),
                        _ => None,
                    };
                }
            }
            Ok(Event::Empty(e)) if in_item && dialect == XmlDialect::Atom => {
                if local_name(e.name().as_ref()) == b"link" {
                    let rel = attr(&e, b"rel");
                    if link.is_empty() && (rel.is_none() || rel.as_deref() == Some("alternate")) {
                        if let Some(href) = attr(&e, b"href") {
                            link = href;
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(f) = field {
                    // quick-xml ≥0.38 не разворачивает сущности внутри Text — только декодирует
                    // байты (здесь UTF-8, вход `from_str`). Сами сущности приходят отдельными
                    // GeneralRef-событиями (ниже), так что поле собирается тем же итогом, что
                    // давал `unescape()` до миграции.
                    let s = t.decode().map_err(|e| NewsError::Parse(e.to_string()))?;
                    push_field(f, &s, &mut title, &mut link, &mut date, &mut desc);
                }
            }
            // Сущность внутри активного поля (`&amp;` в url Хабра, `&lt;`/`&gt;` в escaped-HTML
            // Atom-контента и т.п.): quick-xml ≥0.38 отдаёт её отдельным событием с ИМЕНЕМ
            // сущности (без `&`/`;`). Восстанавливаем `&name;` и декодируем ТЕМ ЖЕ
            // `decode_entities`, что и остальной конвейер выжимки. Семантика vs 0.37-`unescape()`:
            // на ВАЛИДНЫХ предопределённых (lt/gt/amp/quot/apos) и числовых сущностях итог
            // идентичен; НЕВАЛИДНЫЕ по XML, ронявшие до миграции ВЕСЬ фид ошибкой парсинга,
            // теперь мягко проходят правилами выжимки (`&unknown;`/out-of-range — литералом,
            // `&nbsp;` — пробелом, `&#X26;` — декодом) — осознанный availability-выигрыш,
            // см. пин-тест. `&#0;`/`&#x0;` (NUL) decode_entities отвергает литералом (до
            // миграции NUL в полях был недостижим — unescape() его отвергал; сохраняем).
            Ok(Event::GeneralRef(r)) => {
                if let Some(f) = field {
                    let name = r.decode().map_err(|e| NewsError::Parse(e.to_string()))?;
                    let s = decode_entities(&format!("&{name};"));
                    push_field(f, &s, &mut title, &mut link, &mut date, &mut desc);
                }
            }
            Ok(Event::CData(t)) => {
                if let Some(f) = field {
                    let s = String::from_utf8_lossy(&t.into_inner()).into_owned();
                    push_field(f, &s, &mut title, &mut link, &mut date, &mut desc);
                }
            }
            Ok(Event::End(e)) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if name == item_tag {
                    in_item = false;
                    let url = link.trim().to_string();
                    let t = html_to_text(&title, usize::MAX);
                    if !url.is_empty() && !t.is_empty() {
                        out.push(NewsEntry {
                            source_id: source_id.to_string(),
                            url,
                            title: t,
                            published_at: parse_date(date.trim()),
                            excerpt: html_to_text(&desc, EXCERPT_MAX_CHARS),
                            comments_url: None,
                        });
                    }
                } else {
                    field = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(NewsError::Parse(e.to_string())),
            Ok(_) => {}
        }
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum Field {
    Title,
    Link,
    Date,
    Desc,
}

fn push_field(
    f: Field,
    s: &str,
    title: &mut String,
    link: &mut String,
    date: &mut String,
    desc: &mut String,
) {
    match f {
        Field::Title => title.push_str(s),
        Field::Link => link.push_str(s),
        Field::Date => date.push_str(s),
        Field::Desc => desc.push_str(s),
    }
}

/// Имя тега без namespace-префикса (`dc:date` → `date`).
fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        // 0.40 переименовал `unescape_value` → `normalized_value(version)` (XML attr-value
        // normalization; для href/rel-значений без внутренних пробелов итог тот же). Фиды —
        // XML 1.0 (`Implicit1_0` = дефолт); значения атрибутов у нас UTF-8 из `from_str`.
        .and_then(|a| {
            a.normalized_value(quick_xml::XmlVersion::default())
                .ok()
                .map(|v| v.into_owned())
        })
}

// ── JSON (HF daily_papers / HN Algolia) ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct HfDaily {
    title: Option<String>,
    summary: Option<String>,
    #[serde(rename = "publishedAt")]
    published_at: Option<String>,
    paper: Option<HfPaper>,
}
#[derive(Deserialize)]
struct HfPaper {
    id: Option<String>,
    #[serde(rename = "publishedAt")]
    published_at: Option<String>,
}

fn parse_hf(source_id: &str, body: &str) -> Result<Vec<NewsEntry>, NewsError> {
    let items: Vec<HfDaily> =
        serde_json::from_str(body).map_err(|e| NewsError::Parse(e.to_string()))?;
    Ok(items
        .into_iter()
        .filter_map(|i| {
            let id = i.paper.as_ref()?.id.clone()?;
            let title = i.title?.trim().to_string();
            if title.is_empty() {
                return None;
            }
            let date = i
                .published_at
                .or_else(|| i.paper.and_then(|p| p.published_at))
                .unwrap_or_default();
            Some(NewsEntry {
                source_id: source_id.to_string(),
                url: format!("https://huggingface.co/papers/{id}"),
                title,
                published_at: parse_date(&date),
                excerpt: html_to_text(&i.summary.unwrap_or_default(), EXCERPT_MAX_CHARS),
                comments_url: None,
            })
        })
        .collect())
}

#[derive(Deserialize)]
struct HnResp {
    hits: Vec<HnHit>,
}
#[derive(Deserialize)]
struct HnHit {
    title: Option<String>,
    url: Option<String>,
    created_at_i: Option<i64>,
    #[serde(rename = "objectID")]
    object_id: Option<String>,
    story_text: Option<String>,
}

fn parse_hn(source_id: &str, body: &str) -> Result<Vec<NewsEntry>, NewsError> {
    let resp: HnResp = serde_json::from_str(body).map_err(|e| NewsError::Parse(e.to_string()))?;
    Ok(resp
        .hits
        .into_iter()
        .filter_map(|h| {
            let title = h.title?.trim().to_string();
            if title.is_empty() {
                return None;
            }
            // url = отправленная ссылка (Show HN → github и т.п.). У текстовых постов её нет →
            // url становится самим обсуждением. discussion — ссылка на HN-тред (нужен objectID).
            let discussion = h
                .object_id
                .as_ref()
                .map(|id| format!("https://news.ycombinator.com/item?id={id}"));
            // comments_url показываем, только если url ОТЛИЧАЕТСЯ от обсуждения (есть внешняя ссылка).
            let (url, comments_url) = match h.url.filter(|u| !u.trim().is_empty()) {
                Some(u) => (u, discussion),
                None => (discussion.clone()?, None),
            };
            Some(NewsEntry {
                source_id: source_id.to_string(),
                url,
                title,
                published_at: h.created_at_i.unwrap_or(0),
                excerpt: html_to_text(&h.story_text.unwrap_or_default(), EXCERPT_MAX_CHARS),
                comments_url,
            })
        })
        .collect())
}

// ── Выжимка: HTML → plain-text ───────────────────────────────────────────────────────────────

/// Срезает теги, декодирует базовые энтити, схлопывает пробелы, обрезает по границе символа.
/// Не парсер HTML — выжимка для фильтра/LLM-входа (контент недоверенный, никуда не рендерится).
fn html_to_text(html: &str, max_chars: usize) -> String {
    let mut text = String::with_capacity(html.len().min(2 * EXCERPT_MAX_CHARS));
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let decoded = decode_entities(&text);
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Базовые именованные + числовые (`&#NNN;`/`&#xNN;`) энтити — то, что реально встречается
/// в выжимках фидов. Неизвестные остаются как есть (текст, не рендер).
/// `pub(crate)`: переиспользуется извлечением абзацев статьи (NF-6, `news::article`).
pub(crate) fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // Энтити короткие: `;` дальше 12 байт → это просто амперсанд в тексте (срезать слайсом
        // нельзя — граница может попасть внутрь многобайтового символа, кириллица Хабра).
        let Some(end) = rest.find(';').filter(|&e| e <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let decoded: Option<char> = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32)
                // NUL отвергаем (→ литерал `&#0;`, как невалидные): XML запрещает `&#0;`, а с
                // миграцией на quick-xml 0.41 эта ветка стала достижима для ПОЛЕЙ фида, включая
                // url (GeneralRef-arm parse_xml); до миграции unescape() отвергал NUL целиком.
                .filter(|&c| c != '\0'),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ── Даты: RFC 3339 (Atom/JSON) и RFC 2822 (RSS) без chrono ──────────────────────────────────

/// Унифицированный парс даты записи: RFC 3339 → RFC 2822; непарсимое → 0 (запись не теряем,
/// она просто сортируется как самая старая — no silent drop).
fn parse_date(s: &str) -> i64 {
    if s.is_empty() {
        return 0;
    }
    rfc3339_to_unix(s)
        .or_else(|| rfc2822_to_unix(s))
        .unwrap_or(0)
}

/// `2026-06-10T08:30:00Z` · с `.123` · с `±hh:mm`.
fn rfc3339_to_unix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    if let Some(dot) = rest.strip_prefix('.') {
        let n = dot.bytes().take_while(|b| b.is_ascii_digit()).count();
        rest = &dot[n..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6)?.parse::<i64>().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

/// `Tue, 10 Jun 2026 08:00:00 GMT` (день недели опционален; зона — GMT/UT/UTC/±hhmm).
fn rfc2822_to_unix(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let start = usize::from(parts.first()?.ends_with(','));
    let d: i64 = parts.get(start)?.parse().ok()?;
    let mo = match *parts.get(start + 1)? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let y: i64 = parts.get(start + 2)?.parse().ok()?;
    let mut hms = parts.get(start + 3)?.split(':');
    let h: i64 = hms.next()?.parse().ok()?;
    let mi: i64 = hms.next()?.parse().ok()?;
    let sec: i64 = hms.next().unwrap_or("0").parse().ok()?;
    let offset = match parts.get(start + 4).copied() {
        None | Some("GMT") | Some("UT") | Some("UTC") | Some("Z") => 0,
        Some(tz) => {
            let sign = match tz.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => 0, // именованные зоны (EST и т.п.) — трактуем как UTC, точность дня достаточна
            };
            if sign == 0 {
                0
            } else {
                let oh = tz.get(1..3)?.parse::<i64>().ok()?;
                let om = tz.get(3..5)?.parse::<i64>().ok()?;
                sign * (oh * 3600 + om * 60)
            }
        }
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::news::FeedKind;

    const OPENAI_RSS: &str = include_str!("fixtures/openai_rss.xml");
    const HABR_RSS: &str = include_str!("fixtures/habr_ai_rss.xml");
    const WILLISON_ATOM: &str = include_str!("fixtures/willison_atom.xml");
    const LLAMACPP_ATOM: &str = include_str!("fixtures/llamacpp_releases_atom.xml");
    const HF_JSON: &str = include_str!("fixtures/hf_daily_papers.json");
    const HN_JSON: &str = include_str!("fixtures/hn_algolia.json");

    /// AC-NF-1: каждая фикстура реального фида (заморожена 2026-06-10) даёт нормализованные
    /// записи: непустые url/title, валидная дата, выжимка без HTML и в лимите.
    #[test]
    fn parses_all_fixture_kinds_to_normalized_entries() {
        let cases = [
            (FeedKind::Rss, "openai", OPENAI_RSS),
            (FeedKind::Rss, "habr-ai", HABR_RSS),
            (FeedKind::Atom, "willison", WILLISON_ATOM),
            (FeedKind::Atom, "llama-cpp", LLAMACPP_ATOM),
            (FeedKind::HfDailyPapers, "hf-papers", HF_JSON),
            (FeedKind::HnAlgolia, "hn", HN_JSON),
        ];
        for (kind, id, body) in cases {
            let entries = parse_feed(kind, id, body).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(entries.len(), 4, "{id}: фикстура подрезана до 4 записей");
            for e in &entries {
                assert!(e.url.starts_with("http"), "{id}: url «{}»", e.url);
                assert!(!e.title.trim().is_empty(), "{id}: пустой title");
                assert!(
                    e.published_at > 1_500_000_000,
                    "{id}: дата «{}»",
                    e.published_at
                );
                assert!(
                    !e.excerpt.contains('<'),
                    "{id}: HTML в выжимке: {}",
                    e.excerpt
                );
                assert!(
                    e.excerpt.chars().count() <= EXCERPT_MAX_CHARS + 1,
                    "{id}: выжимка не в лимите"
                );
                assert_eq!(e.source_id, id);
            }
        }
    }

    /// NF-6 хвост: у HN-айтема с внешней ссылкой (Show HN) `url` = отправленная ссылка, а
    /// `comments_url` = HN-тред по objectID; у текстового поста (url=None) `url` САМ становится
    /// обсуждением, `comments_url` = None (без дубль-кнопки).
    #[test]
    fn hn_carries_discussion_link_only_for_external_url() {
        let entries = parse_feed(FeedKind::HnAlgolia, "hn", HN_JSON).unwrap();
        // Show HN на внешний сайт: url внешний, comments_url ведёт на тред.
        let show = entries
            .iter()
            .find(|e| e.url.contains("eatmydata.ai"))
            .expect("Show HN с внешним url");
        assert_eq!(
            show.comments_url.as_deref(),
            Some("https://news.ycombinator.com/item?id=48472867")
        );
        // Текстовый пост (url отсутствовал): url == обсуждение, comments_url пуст.
        let textless = entries
            .iter()
            .find(|e| e.url.contains("item?id=48472158"))
            .expect("текстовый HN-пост → url=обсуждение");
        assert_eq!(textless.comments_url, None);
    }

    /// Спот-чеки специфики: кириллица Хабра цела; релиз llama.cpp ведёт на /releases/tag/;
    /// HF-url строится из paper.id.
    #[test]
    fn dialect_specific_fields_survive() {
        let habr = parse_feed(FeedKind::Rss, "habr-ai", HABR_RSS).unwrap();
        assert!(
            habr.iter().any(|e| e
                .title
                .chars()
                .any(|c| ('а'..='я').contains(&c.to_ascii_lowercase())
                    || c.is_alphabetic() && !c.is_ascii())),
            "хабр: русский заголовок"
        );
        let rel = parse_feed(FeedKind::Atom, "llama-cpp", LLAMACPP_ATOM).unwrap();
        assert!(
            rel.iter().all(|e| e.url.contains("/releases/tag/")),
            "atom-link из href"
        );
        let hf = parse_feed(FeedKind::HfDailyPapers, "hf-papers", HF_JSON).unwrap();
        assert!(hf
            .iter()
            .all(|e| e.url.starts_with("https://huggingface.co/papers/")));
    }

    /// HN: пост без url (Ask HN) получает ссылку на обсуждение, а не отбрасывается.
    #[test]
    fn hn_text_post_falls_back_to_discussion_url() {
        let body = r#"{ "hits": [ { "title": "Ask HN: local LLM?", "url": null,
            "created_at_i": 1765000000, "objectID": "123", "story_text": "<p>which &amp; why</p>" } ] }"#;
        let e = parse_feed(FeedKind::HnAlgolia, "hn", body).unwrap();
        assert_eq!(e[0].url, "https://news.ycombinator.com/item?id=123");
        assert_eq!(e[0].excerpt, "which & why");
    }

    /// AC-NF-1: битый вход — типизированная ошибка (источник пропустится с видимой пометкой),
    /// не паника и не пустой успех.
    #[test]
    fn malformed_input_is_typed_error() {
        assert!(
            parse_feed(FeedKind::Rss, "x", "<rss><channel><item><title>oops").is_err()
                || parse_feed(FeedKind::Rss, "x", "<rss><channel><item><title>oops")
                    .unwrap()
                    .is_empty()
        );
        assert!(parse_feed(FeedKind::HfDailyPapers, "x", "{not json").is_err());
        assert!(
            parse_feed(FeedKind::HnAlgolia, "x", "[]").is_err(),
            "не та форма (нет hits)"
        );
    }

    /// Даты обоих стандартов и крайние формы; непарсимое → 0 (запись не теряется).
    #[test]
    fn date_parsers_cover_feed_formats() {
        assert_eq!(rfc3339_to_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_unix("1970-01-01T02:00:00+02:00"), Some(0));
        assert_eq!(
            rfc3339_to_unix("2026-06-10T08:30:15.123Z"),
            rfc3339_to_unix("2026-06-10T08:30:15Z")
        );
        assert_eq!(rfc2822_to_unix("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            rfc2822_to_unix("Tue, 10 Jun 2026 08:00:00 +0200"),
            rfc3339_to_unix("2026-06-10T08:00:00+02:00")
        );
        assert_eq!(parse_date("когда-нибудь"), 0);
    }

    /// Выжимка: теги срезаны, энтити декодированы, пробелы схлопнуты, обрезка по символам.
    #[test]
    fn html_to_text_cleans_and_truncates() {
        let s = html_to_text(
            "<p>A &amp; B&nbsp;&lt;tag&gt; &#1090;&#1077;&#1089;&#1090;</p>",
            100,
        );
        assert_eq!(s, "A & B <tag> тест");
        let long = html_to_text(&"я".repeat(600), 10);
        assert_eq!(long.chars().count(), 11, "10 символов + многоточие");
        assert!(long.ends_with('…'));
    }

    /// Регресс миграции quick-xml 0.41 (сущности в Text → отдельные `GeneralRef`): сущность
    /// внутри НЕ-CDATA поля должна разворачиваться байт-в-байт как раньше. `&amp;` в `<link>`
    /// RSS даёт литеральный `&` в url (не `&amp;`, не «склейку» с потерей символа), а
    /// escaped-HTML в Atom-`<content>` — декодируется и чистится в plain-text выжимку.
    #[test]
    fn entities_in_text_nodes_decode_like_pre_migration() {
        let rss = r#"<rss><channel><item>
            <title>T&amp;C</title>
            <link>https://e.com/a?x=1&amp;y=2&amp;z=3</link>
            <pubDate>Tue, 10 Jun 2026 08:00:00 GMT</pubDate>
        </item></channel></rss>"#;
        let e = parse_feed(FeedKind::Rss, "s", rss).unwrap();
        assert_eq!(
            e[0].url, "https://e.com/a?x=1&y=2&z=3",
            "amp в url развёрнут"
        );
        assert_eq!(e[0].title, "T&C", "amp в заголовке развёрнут");

        let atom = concat!(
            "<feed><entry><title>R</title>",
            "<link href=\"https://e.com/x?a=1&amp;b=2\" rel=\"alternate\"/>",
            "<updated>2026-06-10T08:00:00Z</updated>",
            "<content type=\"html\">&lt;p&gt;A &amp;amp; B &amp;lt;ok&amp;gt;&lt;/p&gt;</content>",
            "</entry></feed>"
        );
        let a = parse_feed(FeedKind::Atom, "s", atom).unwrap();
        assert_eq!(a[0].url, "https://e.com/x?a=1&b=2", "amp в href развёрнут");
        // `&lt;p&gt;…&lt;/p&gt;` → `<p>…</p>` (снят тегами); `&amp;amp;`→`&amp;`→`&`;
        // `&amp;lt;ok&amp;gt;`→`&lt;ok&gt;`→строка «<ok>» (второй слой энтити выжимки).
        assert_eq!(a[0].excerpt, "A & B <ok>");

        // Числовые char-ref'ы в text-узле тоже приходят GeneralRef'ом ("#38"/"#x26") и
        // декодируются как при 0.37-unescape(); `&unknown;` — литералом (см. ниже).
        let num = r#"<rss><channel><item>
            <title>A &#38; B &#x26; C</title>
            <link>https://e.com/n</link>
        </item></channel></rss>"#;
        let e = parse_feed(FeedKind::Rss, "s", num).unwrap();
        assert_eq!(e[0].title, "A & B & C", "числовые dec/hex развёрнуты");
    }

    /// Пин availability-семантики quick-xml 0.41 (осознанные УЛУЧШЕНИЯ vs 0.37, не тихий дрейф):
    /// (1) голый `&` (без `;`) где угодно — channel-уровень, значение captured-поля, сырой
    /// URL с `?x=1&y=2` — больше НЕ роняет фид целиком (`allow_dangling_amp=true`; в 0.37
    /// сырой `&` внутри захватываемого поля тоже был фатален — unescape() падал);
    /// (2) неизвестная сущность `&unknown;` в поле — литералом, фид жив (в 0.37 — фатально).
    #[test]
    fn dangling_amp_and_unknown_entities_keep_feed_alive() {
        let rss = r#"<rss><channel>
            <title>News & Views</title>
            <category>AI & ML</category>
            <item>
                <title>Q &unknown; A & B</title>
                <link>https://e.com/a?x=1&y=2</link>
                <pubDate>Tue, 10 Jun 2026 08:00:00 GMT</pubDate>
            </item>
        </channel></rss>"#;
        let e = parse_feed(FeedKind::Rss, "s", rss).unwrap();
        assert_eq!(e.len(), 1, "фид с голыми & жив целиком");
        assert_eq!(
            e[0].url, "https://e.com/a?x=1&y=2",
            "сырой & в url — литералом (до миграции РОНЯЛ фид)"
        );
        assert_eq!(
            e[0].title, "Q &unknown; A & B",
            "unknown-сущность и голый & — литералами, без потери текста"
        );
    }

    /// `&#0;`/`&#x0;` (NUL): decode_entities отвергает литералом — NUL не должен попадать в
    /// поля записи (включая url), как и до миграции (0.37-unescape() отвергал NUL фатально;
    /// теперь — мягкий литерал, содержимое поля без управляющего символа).
    #[test]
    fn nul_char_refs_are_rejected_as_literals() {
        assert_eq!(decode_entities("a&#0;b"), "a&#0;b", "dec NUL — литерал");
        assert_eq!(decode_entities("a&#x0;b"), "a&#x0;b", "hex NUL — литерал");
        // Достижимость через поля фида (GeneralRef-arm): NUL-байта в url нет.
        let rss = r#"<rss><channel><item>
            <title>T</title>
            <link>https://e.com/p&#0;q</link>
        </item></channel></rss>"#;
        let e = parse_feed(FeedKind::Rss, "s", rss).unwrap();
        assert!(!e[0].url.contains('\0'), "NUL не просочился в url");
        assert_eq!(e[0].url, "https://e.com/p&#0;q");
    }
}
