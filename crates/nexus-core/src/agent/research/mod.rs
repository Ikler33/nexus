//! DEEP-RESEARCH (RES) — многораундовый веб-ресёрч как ПАТТЕРН делегирования (порт odysseus
//! `deep_research.py`), а НЕ отдельный движок. Строится поверх субагентов (SUBAGENTS), существующих
//! `web.search`/`web.fetch` и actuator-гейта записи в vault. Всё default-OFF (`ai.research.enabled`).
//!
//! # RES-1 (этот срез) — чистый субстрат, БЕЗ I/O и сети
//! Промпт-шаблоны ([`prompts`]), толерантные парсеры плана/запросов/стоп-решения ([`plan`]/[`query`]/
//! [`stop`]) и дедуп источников ([`Finding`]/[`dedup_findings_by_url`]). Все функции чисты над
//! инъектируемыми входами (LLM-ответ как `&str`, дата как unix-секунды) → тестируемы offline. Реальные
//! LLM-вызовы, fan-out воркеров и запись отчёта добавят RES-2/3/4. Конфиг — [`crate::ai::ResearchConfig`].
//!
//! ## Общие толерантные парс-хелперы
//! Локальные reasoning-модели (Qwen) обрамляют ответ `<think>…</think>`, markdown-фенсами и эхом
//! prompt-примеров. Парсеры здесь fail-closed: при любой неоднозначности берут БЕЗОПАСНЫЙ результат
//! (план → [вопрос]; запросы → пусто; стоп → продолжать), НИКОГДА не паникуют. Без `regex`-зависимости —
//! ручное сканирование сбалансированных JSON-спанов (порт odysseus `_parse_json_array`/`_parse_json_object`
//! «keep last balanced span» против эхо-примеров).

pub mod job;
pub mod orchestrate;
pub mod plan;
pub mod prompts;
pub mod quality;
pub mod query;
pub mod stop;
pub mod tool;
pub mod worker;
pub mod write;

pub use job::{enqueue_deep_research, DeepResearchHandler, KIND_DEEP_RESEARCH};
pub use orchestrate::{run_research, ResearchOutcome, ResearchParams, StopReason};
pub use plan::{parse_plan, ResearchPlan};
pub use prompts::{
    build_final_report_prompt, build_plan_prompt, build_query_prompt, build_stop_prompt,
    build_synthesize_prompt, civil_from_unix, current_date_preamble,
};
pub use quality::is_low_quality;
pub use query::dedup_new_queries;
pub use stop::{parse_stop, StopDecision};
pub use tool::{ResearchContext, ResearchTool};
pub use worker::{research_query, GuardedResearchWeb, ResearchWeb, WebHit, WorkerCfg};

/// Одна находка воркера-ресёрчера (RES-2 заполняет через fenced-JSON; RES-1 определяет форму + дедуп).
/// Поля свободного текста — НЕДОВЕРЕННЫЙ контент веб-страниц; RES-2 обрамит их `injection_marker` перед
/// подачей в любой prompt (анти-prompt-injection, I-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub url: String,
    pub title: String,
    pub summary: String,
    pub evidence: String,
}

/// Канонический дедуп-ключ URL — ЕДИНЫЙ для воркера (`shared_urls`, RES-2) и [`dedup_findings_by_url`]
/// (ревью: два слоя дедупа должны мерить URL ОДИНАКОВО). Парсит URL → нормализует регистр схемы/хоста
/// (даёт парсер) + срезает хвостовые `/` пути; query СОХРАНЯЕТ (так `…/a/?q` == `…/a?q`). Непарсимый →
/// trim + срез хвостового `/` (fallback). Пустой → `""`.
pub(crate) fn normalize_url(url: &str) -> String {
    let t = url.trim();
    if t.is_empty() {
        return String::new();
    }
    match reqwest::Url::parse(t) {
        Ok(mut u) => {
            let path = u.path().to_string();
            if path.len() > 1 {
                u.set_path(path.trim_end_matches('/'));
            }
            let s = u.as_str();
            // косметика рутового слеша: `http://a/` → `http://a` (query/непустой путь не трогаем)
            s.strip_suffix('/').unwrap_or(s).to_string()
        }
        Err(_) => t.trim_end_matches('/').to_string(),
    }
}

/// Дедуп находок по URL: первое вхождение выигрывает, исходный порядок сохранён (порт odysseus
/// `_extract_sources` дедуп). Ключ — [`normalize_url`] (тот же, что у воркера → слои согласованы). В самой
/// `Finding.url` остаётся СЫРОЙ URL (для отображения/цитат). Пустой URL отбрасывается (мусорная находка).
pub fn dedup_findings_by_url(findings: Vec<Finding>) -> Vec<Finding> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(findings.len());
    for f in findings {
        let key = normalize_url(&f.url);
        if key.is_empty() {
            continue;
        }
        if seen.insert(key) {
            out.push(f);
        }
    }
    out
}

/// Снять markdown-фенсы кода (```json … ``` / ``` … ```) если присутствуют (порт odysseus
/// `_strip_code_block`). Возвращает обрезанный по краям текст.
pub(crate) fn strip_code_block(text: &str) -> String {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // отрезать язык-метку до конца первой строки + хвостовой ```
        let after_lang = match rest.find('\n') {
            Some(nl) => &rest[nl + 1..],
            None => rest, // одностроч. ```...``` без перевода — редкий случай
        };
        let body = after_lang.trim_end();
        let body = body.strip_suffix("```").unwrap_or(body);
        return body.trim().to_string();
    }
    t.to_string()
}

/// ASCII-регистронезависимый поиск подстроки `needle` (ОБЯЗАН быть чисто ASCII) в `haystack`; возвращает
/// БАЙТОВОЕ смещение. Тег ASCII → первый совпавший байт ASCII → смещение всегда на char-границе (байт-
/// продолжения UTF-8 `0x80..=0xBF` не равны ASCII). Это устраняет класс багов «оффсеты от lowercased-копии,
/// срез по оригиналу» (case-fold НЕ сохраняет длину: İ/ẞ/Kelvin → паника на multibyte-границе).
fn find_ci_ascii(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Удалить `<think>…</think>`-блоки reasoning-модели (порт odysseus `strip_thinking`). Несколько блоков,
/// регистронезависимо; незакрытый `<think>` без `</think>` → срезается до конца (fail-safe — не тащим
/// reasoning в парс). Работает ПО ОРИГИНАЛУ через [`find_ci_ascii`] (никаких lowercased-копий — иначе
/// case-fold сдвиг длины ломал срезы и ронял парсер на враждебном UTF-8). Возвращает обрезанный текст.
pub(crate) fn strip_thinking(text: &str) -> String {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(open) = find_ci_ascii(rest, OPEN) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]); // текст до <think> (open — на char-границе, ASCII)
        let after_open = &rest[open + OPEN.len()..];
        match find_ci_ascii(after_open, CLOSE) {
            Some(close) => rest = &after_open[close + CLOSE.len()..],
            None => break, // незакрытый блок → отбросить хвост
        }
    }
    out.trim().to_string()
}

/// Все top-level сбалансированные спаны, начинающиеся с `open` и кончающиеся `close` (учитывает строки и
/// escape внутри JSON). Используется парсерами, чтобы выбрать ПОСЛЕДНИЙ валидный (модель часто эхает
/// prompt-пример ПЕРЕД настоящим ответом). Возвращает срезы в порядке появления.
pub(crate) fn balanced_spans(text: &str, open: char, close: char) -> Vec<&str> {
    let bytes: Vec<char> = text.chars().collect();
    // Индексы по символам → байтовые границы для срезов.
    let mut char_to_byte = Vec::with_capacity(bytes.len() + 1);
    let mut b = 0usize;
    for ch in &bytes {
        char_to_byte.push(b);
        b += ch.len_utf8();
    }
    char_to_byte.push(b);

    let mut spans = Vec::new();
    let mut depth = 0i32;
    let mut start_char: Option<usize> = None;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &ch) in bytes.iter().enumerate() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            c if c == open => {
                if depth == 0 {
                    start_char = Some(i);
                }
                depth += 1;
            }
            // close при depth==0 (одинокая `)`/`]`) → guard ложен → падает в `_` (игнор, как и было).
            c if c == close && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start_char.take() {
                        spans.push(&text[char_to_byte[s]..char_to_byte[i + 1]]);
                    }
                }
            }
            _ => {}
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_thinking_removes_blocks() {
        assert_eq!(strip_thinking("<think>hmm</think>YES"), "YES");
        assert_eq!(
            strip_thinking("a <THINK>x</THINK> b <think>y</think> c"),
            "a  b  c".trim()
        );
        // незакрытый → хвост отброшен
        assert_eq!(strip_thinking("keep<think>drop forever"), "keep");
    }

    /// РЕГРЕССИЯ (adversarial CRITICAL-1): case-fold НЕ сохраняет длину (İ/ẞ/Kelvin), а multibyte-хвост
    /// после блока ронял срез по lowercased-оффсету. Враждебный UTF-8 НЕ должен паниковать.
    #[test]
    fn strip_thinking_no_panic_on_hostile_utf8() {
        for poison in [
            "İ<think>X</think>éééé",        // İ (U+0130, 2б) → i̇ (3б): сдвиг +1
            "ẞ<think>x</think>中文中文",    // ẞ → ß: сдвиг −1
            "\u{212A}<think>y</think>🎉🎉", // Kelvin K → k: сдвиг −2 + emoji-хвост
            "<THINK>İẞ</THINK>café",        // регистронезависимо + multibyte внутри
            "プレ<think>думаю</think>пост", // CJK/кириллица вокруг
        ] {
            let _ = strip_thinking(poison); // не паникует = тест прошёл
        }
        // содержимое корректно (İ сохранён, блок вырезан, хвост цел)
        assert_eq!(strip_thinking("İ<think>secret</think>YES"), "İYES");
    }

    #[test]
    fn strip_code_block_unfences() {
        assert_eq!(strip_code_block("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_block("```\n[1,2]\n```"), "[1,2]");
        assert_eq!(strip_code_block("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn balanced_spans_picks_all_top_level() {
        // эхо-пример массива ПЕРЕД настоящим → две спаны, последняя реальная
        let t = "Example: [\"a\", \"b\"]\nAnswer: [\"real one\", \"real two\"]";
        let spans = balanced_spans(t, '[', ']');
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1], "[\"real one\", \"real two\"]");
    }

    #[test]
    fn balanced_spans_ignores_brackets_in_strings() {
        let t = "{\"q\": \"what is [redacted]?\"}";
        let spans = balanced_spans(t, '{', '}');
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0], t);
    }

    #[test]
    fn dedup_findings_first_wins_drops_empty_url() {
        let f = |u: &str| Finding {
            url: u.into(),
            title: "t".into(),
            summary: "s".into(),
            evidence: "e".into(),
        };
        let out = dedup_findings_by_url(vec![f("http://a"), f("http://b"), f("http://a"), f("")]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].url, "http://a");
        assert_eq!(out[1].url, "http://b");
    }

    /// РЕГРЕССИЯ (ревью #5/#6): нормализация — ЕДИНЫЙ ключ; trailing-slash/регистр-хоста/query учтены.
    #[test]
    fn normalize_url_canonicalizes() {
        assert_eq!(normalize_url(" http://a/ "), "http://a");
        assert_eq!(normalize_url("http://a"), "http://a");
        assert_eq!(normalize_url("http://A.COM/Path/"), "http://a.com/Path"); // хост ↓регистр, путь сохр
        assert_eq!(normalize_url("http://a/b/?q=1"), "http://a/b?q=1"); // query сохранён, слеш пути срезан
        assert_eq!(normalize_url(""), "");
        // дедуп по нормали: `http://a` и `http://a/` — один источник
        let f = |u: &str| Finding {
            url: u.into(),
            title: "t".into(),
            summary: "s".into(),
            evidence: "e".into(),
        };
        let out = dedup_findings_by_url(vec![f("http://a"), f("http://a/")]);
        assert_eq!(out.len(), 1, "trailing-slash вариант — тот же источник");
    }
}
