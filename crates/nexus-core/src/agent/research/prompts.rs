//! RES-1: промпт-шаблоны deep-research (порт odysseus `deep_research.py`) + ЧИСТОЕ заземление в дате.
//!
//! Все билдеры — чистые функции `String -> String` (форматирование), без I/O. LLM-вызовы появятся в RES-3
//! (оркестратор). Дата инъектируется как unix-секунды (тест-детерминизм; нет `SystemTime::now()` внутри) —
//! зеркало odysseus `current_date_context(now=None)` с опциональным временем.

/// Месяцы для человекочитаемой даты (no-chrono — в core нет chrono, прецедент `home::stale::days_from_civil`).
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// (year, month 1-12, day 1-31) из unix-секунд UTC. Алгоритм Хиннанта (инверсия
/// `home::stale::days_from_civil`) — чистая арифметика, тестируемо без часов. Делит на дни (floor),
/// время суток отбрасывается. Корректно для дат после 1970 (отрицательные эпохи через `div_euclid`).
pub fn civil_from_unix(secs: i64) -> (i64, u32, u32) {
    let days = secs.div_euclid(86_400); // floor-деление: дни от эпохи (UTC), время суток отброшено
                                        // Хиннант: days→(y,m,d). z — дни от 0000-03-01 эры.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11] (март=0)
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // март=3 → 1..12
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

/// Преамбула, заземляющая планировщик/генератор-запросов в РЕАЛЬНОЙ текущей дате. Без неё локальная модель
/// подставляет год обучения (`"лучшие туториалы 2024"` когда уже 2026). Порт odysseus `current_date_context`.
/// `now_secs` — unix UTC (инъекция → чисто/детерминированно). UTC (в core нет TZ-базы); для заземления ГОДА
/// этого достаточно.
pub fn current_date_preamble(now_secs: i64) -> String {
    let (y, m, d) = civil_from_unix(now_secs);
    let month = MONTHS[(m.clamp(1, 12) - 1) as usize];
    format!(
        "Today's date is {month} {d}, {y} ({y:04}-{m:02}-{d:02}). When a search query needs a year or \
refers to 'latest'/'current'/'this year', use {y} or relative wording — never a year inferred from \
training data.\n\n"
    )
}

/// План-промпт: разложить вопрос на 3-6 подвопросов + ключевые темы + критерий успеха (JSON). Заземлён датой.
pub fn build_plan_prompt(question: &str, now_secs: i64) -> String {
    format!(
        "{date}You are a research strategist. Before searching, analyze this question and create a \
research plan.\n\n\
**Question:** {question}\n\n\
Break this question down:\n\
1. What are the key sub-topics that need to be covered for a comprehensive answer?\n\
2. What specific data points, facts, or perspectives should we look for?\n\
3. What would a complete, high-quality answer include?\n\n\
Return a JSON object with:\n\
- \"sub_questions\": Array of 3-6 specific sub-questions to investigate\n\
- \"key_topics\": Array of key topics/angles to cover\n\
- \"success_criteria\": One sentence describing what a complete answer looks like\n\n\
Return ONLY the JSON object, nothing else.",
        date = current_date_preamble(now_secs),
    )
}

/// Промпт генерации поисковых запросов раунда (broad в раунде 1, gap-fill дальше). Заземлён датой.
pub fn build_query_prompt(
    question: &str,
    research_plan: &str,
    report: &str,
    round_num: u32,
    num_queries: usize,
    now_secs: i64,
) -> String {
    let round_instruction = if round_num <= 1 {
        "This is the first round — generate broad queries covering the main aspects."
    } else {
        "Later round — target the GAPS and unanswered sub-questions in what we know so far."
    };
    let report_block = if report.trim().is_empty() {
        "(nothing yet)"
    } else {
        report
    };
    format!(
        "{date}You are a research assistant planning web searches.\n\n\
**Original question:** {question}\n\n\
**Research plan:**\n{research_plan}\n\n\
**What we know so far:**\n{report_block}\n\n\
**Round:** {round_num}\n\n\
Generate {num_queries} focused search queries that will help answer the question.\n\
{round_instruction}\n\n\
Return ONLY a JSON array of query strings, nothing else.\n\
Example: [\"query one\", \"query two\", \"query three\"]",
        date = current_date_preamble(now_secs),
    )
}

/// Промпт синтеза: вплести находки раунда в эволюционирующий отчёт (порт SYNTHESIZE_PROMPT).
pub fn build_synthesize_prompt(question: &str, report: &str, new_findings: &str) -> String {
    let report_block = if report.trim().is_empty() {
        "(empty — this is the first round)"
    } else {
        report
    };
    format!(
        "You are updating an evolving research report.\n\n\
**Original question:** {question}\n\n\
**Current report:**\n{report_block}\n\n\
**New findings from this round:**\n{new_findings}\n\n\
Integrate the new findings into the existing report. Produce an updated, well-organized report that \
answers the original question as completely as possible given all evidence so far. Remove redundancy, \
resolve contradictions, and maintain logical flow. Keep source URLs as inline citations where relevant.\n\n\
Write only the updated report — no preamble or meta-commentary."
    )
}

/// Стоп-промпт: достаточно ли отчёта (порт STOP_PROMPT). Парсер ответа — [`super::stop::parse_stop`].
pub fn build_stop_prompt(question: &str, report: &str, round_num: u32, max_rounds: u32) -> String {
    format!(
        "You are deciding whether a research report is comprehensive enough.\n\n\
**Original question:** {question}\n\n\
**Current report:**\n{report}\n\n\
**Rounds completed:** {round_num} of {max_rounds}\n\n\
Based on the report so far, do we have enough information to answer the question comprehensively? \
Consider: are the key aspects addressed? are there obvious gaps? is the evidence from multiple sources?\n\n\
If rounds completed is well below the target, prefer continuing unless the report is already exhaustive.\n\n\
Reply with ONLY \"YES\" or \"NO\" followed by a brief one-sentence reason.\n\
Example: \"YES — The report covers all major aspects with evidence from multiple sources.\"\n\
Example: \"NO — We still lack information about the economic impact.\""
    )
}

/// Финальный отчёт: длинный цитированный markdown (порт FINAL_REPORT_PROMPT).
pub fn build_final_report_prompt(question: &str, report: &str) -> String {
    format!(
        "Write a **long, detailed, comprehensive** research report answering this question:\n\n\
**Question:** {question}\n\n\
**All collected evidence and analysis:**\n{report}\n\n\
Requirements:\n\
- Write at MINIMUM 1500 words — a thorough, magazine-quality article\n\
- Use clear ## headings and ### subheadings to organize into logical sections\n\
- Each section should have multiple detailed paragraphs, not just bullet points\n\
- Synthesize and analyze — explain WHY things matter, draw comparisons, provide context\n\
- Include specific data points, numbers, and statistics from the evidence\n\
- Include source URLs as inline citations [like this](url)\n\
- Note where sources agree and where they disagree\n\
- Add a brief executive summary at the top\n\
- End with a clear conclusion that directly answers the question"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-06-23 00:00 UTC = 1_782_172_800 (проверка инверсии Хиннанта).
    const T_2026_06_23: i64 = 1_782_172_800;

    #[test]
    fn civil_from_unix_known_points() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1));
        assert_eq!(civil_from_unix(86_400), (1970, 1, 2));
        // 2000-01-01 = 10957 дней * 86400
        assert_eq!(civil_from_unix(10_957 * 86_400), (2000, 1, 1));
        assert_eq!(civil_from_unix(T_2026_06_23), (2026, 6, 23));
        // время суток отбрасывается (тот же день при +23ч)
        assert_eq!(civil_from_unix(T_2026_06_23 + 23 * 3600), (2026, 6, 23));
    }

    #[test]
    fn date_preamble_present_in_plan_prompt() {
        let p = build_plan_prompt("Best laptops for Rust dev?", T_2026_06_23);
        assert!(
            p.contains("Today's date is June 23, 2026"),
            "human date present"
        );
        assert!(p.contains("2026-06-23"), "iso date present");
        assert!(
            p.contains("never a year inferred from training data"),
            "grounding clause"
        );
        assert!(
            p.contains("Best laptops for Rust dev?"),
            "question interpolated"
        );
        assert!(p.contains("sub_questions"), "plan schema present");
    }

    #[test]
    fn query_prompt_round_instruction_varies() {
        let r1 = build_query_prompt("q", "plan", "", 1, 3, T_2026_06_23);
        assert!(r1.contains("first round"), "round 1 broad");
        let r2 = build_query_prompt("q", "plan", "some report", 2, 3, T_2026_06_23);
        assert!(r2.contains("GAPS"), "later round gap-fill");
        assert!(
            r2.contains("Today's date is"),
            "query prompt also date-grounded"
        );
    }

    #[test]
    fn stop_and_final_prompts_interpolate() {
        let s = build_stop_prompt("q", "rep", 1, 3);
        assert!(s.contains("1 of 3") || s.contains("**Rounds completed:** 1"));
        assert!(s.contains("YES") && s.contains("NO"));
        let f = build_final_report_prompt("q", "rep");
        assert!(f.contains("1500 words"));
    }
}
