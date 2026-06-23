//! RES-4: запись отчёта deep-research в vault ЧЕРЕЗ actuator-гейт (порт odysseus report-write). НИКОГДА не
//! сырой `fs` — строим `Action::note_create` и отдаём в [`ActionDispatcher`] (тот же путь, что `note.create`-
//! инструмент): classify → autonomy/blast-cap/ledger (write-before-act) → atomic_write → обратимо (undo_run).
//! Под `confirm` гейт отдаёт Proposal (не пишет), под `auto` — применяет+аудитит. Гейт — единственный путь.

use crate::actuator::{Action, ActionDispatcher};
use crate::agent::tool::ToolError;

/// Кап длины слага (чтобы путь не разрастался). Имя файла = `<slug>-<date>.md`.
const SLUG_MAX: usize = 60;

/// Слаг из вопроса: lowercase, [a-z0-9] оставляем, прочее → `-`, схлопываем повторы, трим, кап длины.
/// Пусто/мусор → `"research"`. ASCII-only (не-ASCII буквы → `-`; для имени файла этого достаточно).
pub(crate) fn slugify(question: &str) -> String {
    let mut out = String::with_capacity(question.len().min(SLUG_MAX));
    let mut prev_dash = false;
    for ch in question.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= SLUG_MAX {
            break;
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() {
        "research".to_string()
    } else {
        s
    }
}

/// vault-rel путь отчёта: `Research/<slug>-<YYYY-MM-DD>.md`.
pub(crate) fn report_path(question: &str, date_ymd: &str) -> String {
    format!("Research/{}-{}.md", slugify(question), date_ymd)
}

/// БЕЗОПАСНОЕ плоское frontmatter-значение для ОБОИХ читателей (тупой edge-stripper приложения И YAML
/// Obsidian): control-символы / переводы строк / кавычки / двоеточия → пробел, схлопываем, трим, кап длины.
/// БЕЗ обрамляющих кавычек — тупой ридер их не декодирует (ревью #3/#4: эскейпы `\"` протекали). Это
/// ПРОВЕНАНС (пишется один раз, не круглый round-trip), потому лёгкая лоссовость значения допустима.
fn safe_value(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_control() || c == '"' || c == ':' {
                ' '
            } else {
                c
            }
        })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

/// Тело заметки = frontmatter (провенанс) + отчёт. Frontmatter — плоские скаляры (читатель `frontmatter_fields`
/// edge-stripper, без вложенности — см. [[project_nexus_frontmatter_writeback]]).
pub(crate) fn build_body(
    question: &str,
    report: &str,
    sources_count: usize,
    date_ymd: &str,
) -> String {
    format!(
        "---\nsource: nexus-deep-research\nquery: {q}\nsources_count: {n}\ncreated: {d}\n---\n\n{report}\n",
        q = safe_value(question),
        n = sources_count,
        d = date_ymd,
        report = report.trim()
    )
}

/// Записать отчёт через гейт. Возвращает строку-результат инструмента (summary гейта). Путь/тело строятся
/// здесь; применение — ИСКЛЮЧИТЕЛЬНО `dispatcher.apply(Action::note_create)` (нет сырого fs).
pub(crate) async fn write_report(
    dispatcher: &dyn ActionDispatcher,
    question: &str,
    report: &str,
    sources_count: usize,
    date_ymd: &str,
) -> Result<String, ToolError> {
    let path = report_path(question, date_ymd);
    let body = build_body(question, report, sources_count, date_ymd);
    dispatcher.apply(Action::note_create(path, body)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic_and_fallback() {
        assert_eq!(
            slugify("Best laptops for Rust dev?"),
            "best-laptops-for-rust-dev"
        );
        assert_eq!(slugify("   "), "research");
        assert_eq!(slugify("!!!"), "research");
        assert_eq!(slugify("Привет мир"), "research"); // не-ASCII → дефис → трим → fallback
                                                       // кап длины
        let long = "a".repeat(200);
        assert!(slugify(&long).len() <= SLUG_MAX);
    }

    #[test]
    fn report_path_shape() {
        assert_eq!(
            report_path("What is X?", "2026-06-23"),
            "Research/what-is-x-2026-06-23.md"
        );
    }

    #[test]
    fn body_has_frontmatter_provenance() {
        let b = build_body("What is X?", "## Report\n\nbody", 5, "2026-06-23");
        assert!(b.starts_with("---\n"));
        assert!(b.contains("source: nexus-deep-research"));
        assert!(
            b.contains("query: What is X?"),
            "unquoted plain scalar: {b}"
        );
        assert!(b.contains("sources_count: 5"));
        assert!(b.contains("created: 2026-06-23"));
        assert!(b.contains("## Report"));
    }

    #[test]
    fn safe_value_sanitizes_for_both_readers() {
        // кавычки/двоеточия/переводы/control → пробел, схлоп, БЕЗ обрамляющих кавычек (ревью #3/#4)
        assert_eq!(safe_value("a \"b\"\nc:d"), "a b c d");
        assert_eq!(safe_value("plain question"), "plain question");
        assert_eq!(safe_value("tab\there\u{000b}vt"), "tab here vt");
        // кап длины
        assert!(safe_value(&"x ".repeat(300)).chars().count() <= 200);
    }
}
