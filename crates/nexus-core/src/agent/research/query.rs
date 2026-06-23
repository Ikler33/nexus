//! RES-1: разбор сгенерированных поисковых запросов + дедуп против уже использованных (порт odysseus
//! `_parse_json_array` + queries-used-set). Чистые функции — без сети. Сам prompt генерации запросов —
//! [`super::prompts::build_query_prompt`].

use super::{balanced_spans, strip_code_block, strip_thinking};
use std::collections::HashSet;

/// Ключ дедупа запроса: trim + lowercase (так «Rust async», «rust async » — один запрос). Оркестратор
/// (RES-3) кладёт `normalize_query(accepted)` в свой used-set ПОСЛЕ фактического поиска.
pub fn normalize_query(q: &str) -> String {
    q.trim().to_lowercase()
}

/// Распарсить JSON-массив строк-запросов из LLM-ответа. Толерантно: снимает `<think>`/markdown-фенсы, берёт
/// ПОСЛЕДНИЙ валидный непустой `[…]` (модель часто эхает prompt-пример `["query one", …]` ПЕРЕД ответом).
/// Невалид → пусто (fail-closed: раунд просто не добавит новых запросов, оркестратор решит по стопу).
pub fn parse_queries(text: &str) -> Vec<String> {
    let cleaned = strip_code_block(&strip_thinking(text));
    if let Ok(v) = serde_json::from_str::<Vec<String>>(&cleaned) {
        let c = clean(v);
        if !c.is_empty() {
            return c;
        }
    }
    let mut last = None;
    for span in balanced_spans(&cleaned, '[', ']') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(span) {
            let c = clean(v);
            if !c.is_empty() {
                last = Some(c);
            }
        }
    }
    last.unwrap_or_default()
}

fn clean(v: Vec<String>) -> Vec<String> {
    v.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Отфильтровать НОВЫЕ запросы: убрать уже использованные (`used` — множество [`normalize_query`]-ключей) и
/// внутрибатчевые дубли. Порядок сохранён, первая форма выигрывает. `used` НЕ мутируется (вызывающий добавит
/// принятые после поиска — дедуп между раундами).
pub fn dedup_new_queries(candidates: Vec<String>, used: &HashSet<String>) -> Vec<String> {
    let mut local = HashSet::new();
    let mut out = Vec::new();
    for q in candidates {
        let key = normalize_query(&q);
        if key.is_empty() || used.contains(&key) || !local.insert(key) {
            continue;
        }
        out.push(q.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_queries_from_fenced_array() {
        let q = parse_queries("```json\n[\"a\", \"b\", \"c\"]\n```");
        assert_eq!(q, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_queries_picks_last_after_echoed_example() {
        let text =
            "Example: [\"query one\", \"query two\", \"query three\"]\n[\"real q1\", \"real q2\"]";
        assert_eq!(parse_queries(text), vec!["real q1", "real q2"]);
    }

    #[test]
    fn parse_queries_bad_input_empty() {
        for junk in ["", "not an array", "{\"obj\": 1}", "<think>[\"x\"]</think>"] {
            // NB последний: think-блок вырезан → пусто (fail-closed)
            assert!(parse_queries(junk).is_empty(), "junk {junk:?} → empty");
        }
    }

    #[test]
    fn query_dedup_against_used_set() {
        let mut used = HashSet::new();
        used.insert(normalize_query("Rust async"));
        let candidates = vec![
            "rust async".to_string(),    // уже использован (нормализованно) → drop
            "Tokio runtime".to_string(), // новый → keep
            "  RUST ASYNC ".to_string(), // повтор использованного → drop
            "Tokio runtime".to_string(), // внутрибатчевый дубль → drop
            "  ".to_string(),            // пустой → drop
        ];
        let out = dedup_new_queries(candidates, &used);
        assert_eq!(out, vec!["Tokio runtime"]);
    }
}
