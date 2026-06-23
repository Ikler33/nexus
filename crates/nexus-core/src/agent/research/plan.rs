//! RES-1: толерантный парсер плана ресёрча (порт odysseus `_create_plan`/`_parse_json_object` + `_fallback`).
//! Чистая функция над LLM-ответом — без I/O. Fail-closed: любой невалидный/пустой ответ → план из одного
//! подвопроса (сам исходный вопрос), НИКОГДА не паника.

use super::{balanced_spans, strip_code_block, strip_thinking};
use serde::Deserialize;

/// Разобранный план ресёрча. `sub_questions` гарантированно НЕ пуст (минимум сам вопрос) и обрезан до
/// `max_fanout` (под капы делегирования — fan-out воркеров в RES-3 не превысит).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchPlan {
    pub sub_questions: Vec<String>,
    pub key_topics: Vec<String>,
    pub success_criteria: String,
}

/// Сырое представление под serde (все поля опциональны → недостающее = пусто, без ошибки парса).
#[derive(Deserialize, Default)]
struct RawPlan {
    #[serde(default)]
    sub_questions: Vec<String>,
    #[serde(default)]
    key_topics: Vec<String>,
    #[serde(default)]
    success_criteria: String,
}

/// Распарсить план из LLM-ответа. `question` — исходный вопрос (для fail-closed fallback). `max_fanout` —
/// потолок числа подвопросов (≥1; 0 трактуется как 1). Снимает `<think>`/markdown-фенсы, берёт ПОСЛЕДНИЙ
/// валидный `{…}`-объект с непустыми `sub_questions` (анти-эхо prompt-примера), иначе fallback к `[question]`.
pub fn parse_plan(text: &str, question: &str, max_fanout: usize) -> ResearchPlan {
    let cleaned = strip_code_block(&strip_thinking(text));
    let mut plan = match parse_raw(&cleaned) {
        Some(raw) => ResearchPlan {
            sub_questions: raw.sub_questions,
            key_topics: raw.key_topics,
            success_criteria: raw.success_criteria,
        },
        None => fallback(question),
    };

    // Нормализация: выкинуть пустые подвопросы, гарантировать непустоту, обрезать до капа.
    plan.sub_questions.retain(|q| !q.trim().is_empty());
    if plan.sub_questions.is_empty() {
        plan.sub_questions = vec![question.trim().to_string()];
    }
    plan.sub_questions.truncate(max_fanout.max(1));
    plan
}

/// Прямой парс или ПОСЛЕДНИЙ сбалансированный `{…}` с непустыми `sub_questions`.
fn parse_raw(cleaned: &str) -> Option<RawPlan> {
    if let Ok(raw) = serde_json::from_str::<RawPlan>(cleaned) {
        if !raw.sub_questions.is_empty() {
            return Some(raw);
        }
    }
    let mut last_good = None;
    for span in balanced_spans(cleaned, '{', '}') {
        if let Ok(raw) = serde_json::from_str::<RawPlan>(span) {
            if !raw.sub_questions.is_empty() {
                last_good = Some(raw);
            }
        }
    }
    last_good
}

fn fallback(question: &str) -> ResearchPlan {
    ResearchPlan {
        sub_questions: vec![question.trim().to_string()],
        key_topics: Vec::new(),
        success_criteria: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_extracts_subquestions_from_fenced_json() {
        let text =
            "```json\n{\"sub_questions\": [\"What is X?\", \"How does Y work?\", \"Cost of Z?\"], \
\"key_topics\": [\"x\", \"y\"], \"success_criteria\": \"Covers all three.\"}\n```";
        let plan = parse_plan(text, "orig?", 6);
        assert_eq!(plan.sub_questions.len(), 3);
        assert_eq!(plan.sub_questions[0], "What is X?");
        assert_eq!(plan.key_topics, vec!["x", "y"]);
        assert_eq!(plan.success_criteria, "Covers all three.");
    }

    #[test]
    fn parse_plan_bad_json_falls_back_to_question() {
        for junk in [
            "",
            "no json here at all",
            "{ broken",
            "<think>only reasoning</think>",
        ] {
            let plan = parse_plan(junk, "What is the capital of France?", 6);
            assert_eq!(
                plan.sub_questions,
                vec!["What is the capital of France?".to_string()],
                "junk {junk:?} → fallback"
            );
            assert!(plan.key_topics.is_empty());
        }
    }

    #[test]
    fn parse_plan_caps_subquestions_at_max_fanout() {
        let text = "{\"sub_questions\": [\"q1\",\"q2\",\"q3\",\"q4\",\"q5\",\"q6\"]}";
        let plan = parse_plan(text, "orig", 3);
        assert_eq!(plan.sub_questions.len(), 3);
        assert_eq!(plan.sub_questions, vec!["q1", "q2", "q3"]);
    }

    #[test]
    fn parse_plan_ignores_echoed_example_picks_real() {
        // модель эхает prompt-пример (пустой/чужой) ПЕРЕД настоящим ответом
        let text = "Example: {\"sub_questions\": [\"example one\"]}\n\
Here is my plan: {\"sub_questions\": [\"real one\", \"real two\"], \"success_criteria\": \"done\"}";
        let plan = parse_plan(text, "orig", 6);
        assert_eq!(plan.sub_questions, vec!["real one", "real two"]);
        assert_eq!(plan.success_criteria, "done");
    }

    #[test]
    fn parse_plan_caps_even_fallback_min_one() {
        let plan = parse_plan("", "q", 0);
        assert_eq!(plan.sub_questions.len(), 1, "max_fanout 0 → min 1");
    }
}
