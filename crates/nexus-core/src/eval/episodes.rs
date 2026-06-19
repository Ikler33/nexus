//! EP-2 eval-гейт эпизодической памяти (спека `docs/specs/agent-episodic-memory.md` §8):
//! ДЕТЕРМИНИРОВАННЫЙ харнесс **faithfulness** саммари сессии. Главный риск эпизода — ЛОЖНАЯ память:
//! саммари утверждает то, чего в диалоге НЕ было (галлюцинация). Запись эпизода аддитивна/обратима →
//! жёсткий гейт НА ЗАПИСЬ не нужен; но инъекция галлюцинированного саммари в чат = вред. Поэтому гейт
//! БЛОКИРУЕТ включение РЕТРИВАЛА (EP-2 не мержится, пока live-точка `live_episode_summary_meets_gate`
//! не зелёная на актуальной модели).
//!
//! Метрика — доля «верных» саммари: (а) НЕ содержит `forbidden` (сущность, которой в диалоге не было —
//! галлюцинация) И (б) содержит ≥1 `anchor` (ключевая сущность диалога — groundedness, ловит вакуумные/
//! не-по-теме саммари). По образцу `consolidation.rs`: фиктивный предиктор в тестах доказывает, что гейт
//! ЛОВИТ галлюцинацию БЕЗ LLM. При смене модели — рекалибровка (как MEM-8c, [[project_nexus_consolidation_recalibrate]]).

use serde::{Deserialize, Serialize};

/// Порог faithfulness: ≥85% саммари верны (не галлюцинируют + заземлены). Калибруется на данных.
pub const MIN_EPISODE_FAITHFULNESS: f32 = 0.85;
/// Анти-вырожденность: минимум кейсов в наборе (иначе доля на 2-3 кейсах ничего не значит).
pub const MIN_EPISODE_CASES: usize = 20;

/// Реплика диалога в golden-кейсе.
#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptLine {
    /// "user" | "assistant".
    pub role: String,
    pub content: String,
}

/// Один кейс faithfulness: транскрипт сессии + якоря (есть в диалоге) + запрещённые (нет в диалоге).
#[derive(Debug, Clone, Deserialize)]
pub struct EpisodeCase {
    pub name: String,
    pub transcript: Vec<TranscriptLine>,
    /// Ключевые сущности/темы, КОТОРЫЕ ЕСТЬ в диалоге — верное саммари упоминает ≥1 (groundedness).
    pub anchors: Vec<String>,
    /// Правдоподобные сущности, которых в диалоге НЕТ — упоминание = галлюцинация (ложная память).
    #[serde(default)]
    pub forbidden: Vec<String>,
}

/// Golden-набор faithfulness (`eval/episode_eval.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct EpisodeGolden {
    pub cases: Vec<EpisodeCase>,
}

/// Зашитый golden-набор.
pub fn load_episode_golden() -> EpisodeGolden {
    serde_json::from_str(include_str!("../../eval/episode_eval.json"))
        .expect("eval/episode_eval.json валиден")
}

/// Транскрипт кейса в форме, которую принимает `episode::summarize` (`(role, content)`).
pub fn transcript_pairs(case: &EpisodeCase) -> Vec<(String, String)> {
    case.transcript
        .iter()
        .map(|l| (l.role.clone(), l.content.clone()))
        .collect()
}

/// Оценка одного саммари: галлюцинация (есть запрещённое) / off-topic (нет ни одного якоря).
/// Верное = НЕ галлюцинирует И заземлено.
#[derive(Debug, Clone, Copy)]
pub struct SummaryGrade {
    pub faithful: bool,
    pub hallucinated: bool,
    pub off_topic: bool,
}

/// Грейд саммари против кейса (без учёта регистра). Якоря пусты → groundedness не требуем.
pub fn grade_summary(case: &EpisodeCase, summary: &str) -> SummaryGrade {
    let s = summary.to_lowercase();
    let hallucinated = case.forbidden.iter().any(|f| s.contains(&f.to_lowercase()));
    let off_topic =
        !case.anchors.is_empty() && !case.anchors.iter().any(|a| s.contains(&a.to_lowercase()));
    SummaryGrade {
        faithful: !hallucinated && !off_topic,
        hallucinated,
        off_topic,
    }
}

/// Агрегированный отчёт faithfulness.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeReport {
    pub cases: usize,
    pub faithful: usize,
    pub faithfulness: f32,
    /// Саммари с галлюцинацией (упомянули запрещённую сущность) — каждое = ложная память.
    pub hallucinated: usize,
    /// Саммари без единого якоря (вакуумные/не-по-теме).
    pub off_topic: usize,
}

/// Считает метрики по index-выровненным `summaries` и `golden.cases`. Чистая функция.
pub fn evaluate_episodes(summaries: &[String], golden: &EpisodeGolden) -> EpisodeReport {
    let n = golden.cases.len().min(summaries.len());
    let mut faithful = 0usize;
    let mut hallucinated = 0usize;
    let mut off_topic = 0usize;
    for (case, sum) in golden.cases.iter().zip(summaries.iter()) {
        let g = grade_summary(case, sum);
        if g.faithful {
            faithful += 1;
        }
        if g.hallucinated {
            hallucinated += 1;
        }
        if g.off_topic {
            off_topic += 1;
        }
    }
    let faithfulness = if n == 0 {
        0.0
    } else {
        faithful as f32 / n as f32
    };
    EpisodeReport {
        cases: n,
        faithful,
        faithfulness,
        hallucinated,
        off_topic,
    }
}

/// Гейт ретривала эпизодов: faithfulness не ниже порога И достаточно кейсов (анти-вырожденность).
/// Прохождение РАЗБЛОКИРУЕТ инъекцию эпизодов в чат (EP-2).
pub fn meets_episode_gate(report: &EpisodeReport, min_faithfulness: f32) -> bool {
    report.cases >= MIN_EPISODE_CASES && report.faithfulness >= min_faithfulness
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Верный предиктор: саммари = склейка якорей (содержит все якоря, без запрещённого).
    fn faithful_summaries(golden: &EpisodeGolden) -> Vec<String> {
        golden
            .cases
            .iter()
            .map(|c| format!("В разговоре обсуждали {}.", c.anchors.join(", ")))
            .collect()
    }

    #[test]
    fn golden_parses_and_is_well_formed() {
        let g = load_episode_golden();
        assert!(
            g.cases.len() >= MIN_EPISODE_CASES,
            "нужно ≥{MIN_EPISODE_CASES} кейсов, сейчас {}",
            g.cases.len()
        );
        let mut with_forbidden = 0;
        for c in &g.cases {
            assert!(
                c.transcript.len() >= 2,
                "кейс «{}»: транскрипт слишком короткий",
                c.name
            );
            assert!(!c.anchors.is_empty(), "кейс «{}»: нужен ≥1 якорь", c.name);
            for l in &c.transcript {
                assert!(
                    matches!(l.role.as_str(), "user" | "assistant"),
                    "кейс «{}»: роль {} неизвестна",
                    c.name,
                    l.role
                );
            }
            // Якоря должны реально присутствовать в транскрипте (иначе тест groundedness нечестен).
            let joined = c
                .transcript
                .iter()
                .map(|l| l.content.to_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            for a in &c.anchors {
                assert!(
                    joined.contains(&a.to_lowercase()),
                    "кейс «{}»: якорь «{a}» отсутствует в транскрипте",
                    c.name
                );
            }
            // Запрещённые НЕ должны присутствовать в транскрипте (иначе это не галлюцинация).
            for f in &c.forbidden {
                assert!(
                    !joined.contains(&f.to_lowercase()),
                    "кейс «{}»: запрещённое «{f}» есть в транскрипте — не годится как маркер галлюцинации",
                    c.name
                );
            }
            if !c.forbidden.is_empty() {
                with_forbidden += 1;
            }
        }
        // Большинство кейсов должны иметь forbidden — иначе галлюцинация-тест почти ничего не проверяет.
        assert!(
            with_forbidden >= g.cases.len() * 2 / 3,
            "слишком мало кейсов с forbidden ({with_forbidden}/{}) — гейт слабо ловит галлюцинацию",
            g.cases.len()
        );
    }

    #[test]
    fn faithful_predictor_passes_gate() {
        let g = load_episode_golden();
        let report = evaluate_episodes(&faithful_summaries(&g), &g);
        assert_eq!(report.faithfulness, 1.0);
        assert_eq!(report.hallucinated, 0);
        assert_eq!(report.off_topic, 0);
        assert!(meets_episode_gate(&report, MIN_EPISODE_FAITHFULNESS));
    }

    /// КЛЮЧЕВОЙ тест: галлюцинирующий предиктор (вставляет запрещённую сущность) → faithfulness падает →
    /// гейт НЕ проходит. Доказывает, что гейт ловит ложную память БЕЗ LLM.
    #[test]
    fn hallucinating_predictor_fails_gate() {
        let g = load_episode_golden();
        let preds: Vec<String> = g
            .cases
            .iter()
            .map(|c| {
                let extra = c.forbidden.first().cloned().unwrap_or_default();
                format!("Обсуждали {} и {extra}.", c.anchors.join(", "))
            })
            .collect();
        let report = evaluate_episodes(&preds, &g);
        assert!(report.hallucinated > 0, "должны быть галлюцинации");
        assert!(
            report.faithfulness < MIN_EPISODE_FAITHFULNESS,
            "faithfulness={} должна провалить гейт",
            report.faithfulness
        );
        assert!(!meets_episode_gate(&report, MIN_EPISODE_FAITHFULNESS));
    }

    /// Off-topic предиктор (саммари без якорей) → не заземлено → гейт не проходит.
    #[test]
    fn off_topic_predictor_fails_gate() {
        let g = load_episode_golden();
        let preds: Vec<String> = g
            .cases
            .iter()
            .map(|_| "Был какой-то разговор.".to_string())
            .collect();
        let report = evaluate_episodes(&preds, &g);
        assert!(report.off_topic > 0);
        assert!(!meets_episode_gate(&report, MIN_EPISODE_FAITHFULNESS));
    }

    /// Анти-вырожденность: идеальные саммари, но мало кейсов → гейт НЕ проходит (доля на 2 кейсах
    /// бессмысленна).
    #[test]
    fn too_few_cases_fails_gate() {
        let g = EpisodeGolden {
            cases: vec![EpisodeCase {
                name: "t".into(),
                transcript: vec![TranscriptLine {
                    role: "user".into(),
                    content: "кофе".into(),
                }],
                anchors: vec!["кофе".into()],
                forbidden: vec![],
            }],
        };
        let report = evaluate_episodes(&["обсуждали кофе".into()], &g);
        assert_eq!(report.faithfulness, 1.0);
        assert!(
            !meets_episode_gate(&report, MIN_EPISODE_FAITHFULNESS),
            "1 кейс < MIN_EPISODE_CASES — гейт не должен проходить"
        );
    }
}
