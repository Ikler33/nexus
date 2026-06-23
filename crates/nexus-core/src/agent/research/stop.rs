//! RES-1: парсер стоп-решения ресёрча (порт odysseus `_should_stop`). Чистая функция над LLM-ответом.
//! Fail-closed = ПРОДОЛЖАТЬ: только явный ведущий `YES` останавливает; всё прочее (`NO`, мусор, пусто) →
//! `should_stop=false` (лучше лишний раунд, чем оборвать ресёрч на двусмысленности). Толерантно к
//! `<think>…</think>`, markdown-обёртке (`**YES**`), кавычкам и ведущим маркерам.

use super::strip_thinking;

/// Решение «достаточно ли отчёта». `reason` — краткое обоснование модели (после токена YES/NO), может быть
/// пустым.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopDecision {
    pub should_stop: bool,
    pub reason: String,
}

/// Снять ведущие пробелы и markdown/кавычки-маркеры (`* _ ` `# > - " '`) — чтобы добраться до YES/NO.
fn strip_lead(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_whitespace() || "*_`\"'#>-".contains(c))
}

/// Начинается ли `s` с ASCII-ТОКЕНА `token` НА ГРАНИЦЕ СЛОВА (следом — конец строки либо не-буквенно-
/// цифровой символ), регистронезависимо. Так «YES» матчит, а «YESTERDAY»/«YESolutely» — НЕТ (это не вердикт
/// YES, иначе fail-OPEN: ресёрч оборвётся рано). `token` обязан быть ASCII (тогда `s[token.len()..]` — на
/// char-границе). `&&` короткозамкнут: если префикс не совпал, `s[token.len()..]` не вычисляется (нет паники
/// на коротком `s`).
fn starts_token(s: &str, token: &str) -> bool {
    s.get(..token.len())
        .map(|p| p.eq_ignore_ascii_case(token))
        .unwrap_or(false)
        && !s[token.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric())
}

/// Распарсить стоп-ответ. Останавливаемся ТОЛЬКО при ведущем ТОКЕНЕ `YES` (после снятия think/markdown).
pub fn parse_stop(text: &str) -> StopDecision {
    let cleaned = strip_thinking(text);
    let head = strip_lead(&cleaned);
    StopDecision {
        should_stop: starts_token(head, "YES"),
        reason: extract_reason(head),
    }
}

/// Остаток первой непустой строки после ТОКЕНА YES/NO (граница слова), очищенный от ведущих разделителей
/// (`— - : . ,`). «YESTERDAY» не считается токеном → не срезается.
fn extract_reason(head: &str) -> String {
    let line = head
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let rest = if starts_token(line, "YES") {
        &line[3..]
    } else if starts_token(line, "NO") {
        &line[2..]
    } else {
        line
    };
    rest.trim_start_matches(|c: char| c.is_whitespace() || "—-:.,*\"'".contains(c))
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_parses_yes_after_think_block_and_markdown() {
        let d =
            parse_stop("<think>let me weigh the gaps...</think>\n**YES** — comprehensive enough.");
        assert!(d.should_stop);
        assert_eq!(d.reason, "comprehensive enough.");
    }

    #[test]
    fn stop_no_means_continue_with_reason() {
        let d = parse_stop("NO — we still lack economic impact data.");
        assert!(!d.should_stop);
        assert_eq!(d.reason, "we still lack economic impact data.");
    }

    #[test]
    fn stop_failclosed_continues_on_ambiguous() {
        for junk in ["", "maybe?", "<think>YES</think>", "I think we are done"] {
            // NB 3rd: YES внутри think → вырезан → ведущего YES нет → continue (fail-closed)
            assert!(!parse_stop(junk).should_stop, "junk {junk:?} → continue");
        }
    }

    #[test]
    fn stop_tolerates_quotes_and_yes_dot() {
        assert!(parse_stop("\"YES. Done.\"").should_stop);
        assert!(parse_stop("Yes, the report is complete").should_stop);
    }

    /// РЕГРЕССИЯ (adversarial MAJOR-2): «YES» как ПРЕФИКС без границы слова был fail-OPEN — «YESTERDAY…»
    /// обрывал ресёрч. Теперь требуется граница слова.
    #[test]
    fn stop_word_boundary_not_prefix() {
        for not_yes in [
            "YESTERDAY we covered a lot, keep digging",
            "YESolutely more to explore",
            "YESritagain — continue",
        ] {
            assert!(
                !parse_stop(not_yes).should_stop,
                "{not_yes:?} НЕ токен YES → continue"
            );
        }
        // настоящий вердикт YES (граница слова) — останавливаемся
        assert!(parse_stop("YES the report is complete").should_stop);
        assert!(parse_stop("YES").should_stop);
        // reason не отгрызает хвост у не-токена
        assert_eq!(
            parse_stop("YESTERDAY notes").reason,
            "YESTERDAY notes",
            "не-токен YES не срезается из reason"
        );
    }
}
