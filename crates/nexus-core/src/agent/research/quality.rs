//! RES-2: фильтр низкокачественных находок (порт odysseus `research_utils.is_low_quality` +
//! `LOW_QUALITY_MARKERS`). Чистая функция — отсев перед сбором: пустой/слишком короткий summary или
//! boilerplate-маркер → находка отбрасывается воркером.

/// Маркеры бесполезного summary (порт odysseus, УЖЕСТОЧЁН по ревью). ОТЛИЧИТЕЛЬНЫЕ фразы — не голые
/// «cookie»/«copyright» и не общеграмматические обрывки («does not contain»/«not relevant to»/«insufficient
/// to»), которые ложно срабатывали на легитимных находках, где это сам ПРЕДМЕТ («this API does not contain
/// a delete method», «the study is insufficient to prove X»). Цель — ловить boilerplate/«нет данных»-вердикты
/// модели, не отсекая валидное содержание.
const LOW_QUALITY_MARKERS: &[&str] = &[
    "content is insufficient",
    "no substantive data",
    "no relevant information",
    "unable to extract",
    "completely unrelated",
    "boilerplate",
    "footer text",
    "cookie consent",
    "cookie banner",
    "cookie notice",
    "copyright notice",
    "copyright footer",
    "all rights reserved",
];

/// Минимальная содержательная длина summary (anti-«ok»/обрывок). odysseus полагается на маркеры; добавляем
/// консервативный флор (defense-in-depth — очень короткий summary бесполезен для отчёта).
const MIN_SUMMARY_CHARS: usize = 20;

/// Низкокачественна ли находка по её summary? Пусто/короче [`MIN_SUMMARY_CHARS`] → да; иначе содержит ли
/// (регистронезависимо) low-quality-маркер. Воркер отбрасывает такие находки.
pub fn is_low_quality(summary: &str) -> bool {
    let s = summary.trim();
    if s.chars().count() < MIN_SUMMARY_CHARS {
        return true;
    }
    let low = s.to_lowercase();
    LOW_QUALITY_MARKERS.iter().any(|m| low.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_quality_empty_or_short() {
        assert!(is_low_quality(""));
        assert!(is_low_quality("   "));
        assert!(is_low_quality("too short"));
    }

    #[test]
    fn low_quality_boilerplate_markers() {
        assert!(is_low_quality(
            "This page contains no relevant information about the query whatsoever."
        ));
        assert!(is_low_quality(
            "The content is insufficient to answer; mostly a cookie consent banner."
        ));
        assert!(is_low_quality(
            "Unable to extract anything meaningful from this footer text."
        ));
    }

    /// РЕГРЕССИЯ (ревью #4): общеграмматические обрывки больше НЕ маркеры — легит. находки с ними проходят.
    #[test]
    fn legit_finding_with_grammatical_fragment_passes() {
        assert!(!is_low_quality(
            "The HTTP/2 spec does not contain a mandatory server-push requirement; it is optional."
        ));
        assert!(!is_low_quality(
            "This dosage is insufficient to reach therapeutic levels in adults per the phase-3 trial."
        ));
        assert!(!is_low_quality(
            "Section 4 is not relevant to minors, but section 5 applies to users of all ages."
        ));
    }

    #[test]
    fn substantive_summary_passes() {
        assert!(!is_low_quality(
            "Rust's async model uses futures polled by an executor; Tokio is the dominant runtime, \
             offering a multi-threaded scheduler and rich I/O primitives."
        ));
        // легитимная статья ПРО cookies (предмет) — голое слово не маркер, проходит
        assert!(!is_low_quality(
            "The EU cookie law (ePrivacy Directive) requires sites to obtain consent before storing \
             non-essential cookies, with fines for non-compliance."
        ));
    }
}
