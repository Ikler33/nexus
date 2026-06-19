//! EVAL-AI — нулевой слайс ПЕРЕД AI-2c (спека §14.3, MAJOR): ДЕТЕРМИНИРОВАННЫЙ харнесс качества
//! closed-vocabulary классификации тегов (для авто-тега AI-2c). Метрики — МИКРО precision/recall/F1 на
//! уровне (заметка → множество тегов): TP/FP/FN суммируются по всем кейсам. Гейт — пороги
//! `precision ≥ 0.8`, `recall ≥ 0.5` (спека) ПЛЮС жёсткий closed-vocab-инвариант: любой предсказанный тег
//! ВНЕ словаря — провал (suggested_new ВЫКЛ). Фиктивный классификатор в тестах доказывает, что гейт ловит
//! регресс БЕЗ LLM — то есть сам гейт тестируем (смысл «нулевого слайса»). AI-2c подставит сюда реальный
//! `chat_util`-классификатор и сверит его отчёт с этими порогами.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Порог микро-precision гейта авто-тега (спека §10 A4).
pub const MIN_PRECISION: f32 = 0.8;
/// Порог микро-recall гейта авто-тега (спека §10 A4).
pub const MIN_RECALL: f32 = 0.5;

/// Один кейс: заметка + ОЖИДАЕМЫЕ теги (gold) из закрытого словаря. `body` — содержимое заметки для
/// LIVE-классификатора (AI-2c live-тест); детерминированные гейт-тесты `body` не используют (default "").
#[derive(Debug, Clone, Deserialize)]
pub struct TagCase {
    pub path: String,
    pub gold: Vec<String>,
    #[serde(default)]
    pub body: String,
}

/// Golden-набор тег-классификации: закрытый словарь допустимых тегов + кейсы.
#[derive(Debug, Clone, Deserialize)]
pub struct TagGolden {
    pub vocabulary: Vec<String>,
    pub cases: Vec<TagCase>,
}

/// Зашитый golden-набор авто-тега (`eval/tag_golden.json`).
pub fn load_tag_golden() -> TagGolden {
    serde_json::from_str(include_str!("../../eval/tag_golden.json"))
        .expect("eval/tag_golden.json валиден")
}

/// Микро-усреднённый отчёт multi-label тег-классификации. `out_of_vocab` — число предсказанных тегов вне
/// словаря (closed-vocab-нарушение; они НЕ зачитываются как TP и идут в FP).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClassifyReport {
    pub tp: usize,
    pub fp: usize,
    pub fn_count: usize,
    pub precision: f32,
    pub recall: f32,
    pub f1: f32,
    pub cases: usize,
    pub out_of_vocab: usize,
}

/// Предсказание тегов для одной заметки.
pub type Prediction = (String, HashSet<String>);

/// Считает микро-метрики по парам (предсказание, gold), сматчив по пути. Теги вне `vocabulary` —
/// closed-vocab-нарушение: считаются как FP И инкрементят `out_of_vocab` (не как TP, даже если случайно
/// совпали бы с gold — gold ⊆ словарь по построению). Кейс gold без предсказания → его теги = FN.
/// precision = TP/(TP+FP), recall = TP/(TP+FN), F1 = 2PR/(P+R); нулевой знаменатель → 0.0 (детерминизм).
/// ПРЕДУСЛОВИЕ: пути в `predictions` и в `gold` уникальны (HashMap-матч — дубль пути схлопнулся бы
/// last-wins). Для golden-фикстуры/одно-предсказание-на-заметку это держится.
pub fn evaluate_tags(
    predictions: &[Prediction],
    gold: &[(String, Vec<String>)],
    vocabulary: &HashSet<String>,
) -> ClassifyReport {
    let gold_map: HashMap<&str, HashSet<&str>> = gold
        .iter()
        .map(|(p, tags)| (p.as_str(), tags.iter().map(String::as_str).collect()))
        .collect();
    let pred_map: HashMap<&str, &HashSet<String>> =
        predictions.iter().map(|(p, t)| (p.as_str(), t)).collect();

    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut fn_count = 0usize;
    let mut out_of_vocab = 0usize;

    // Все пути из обоих источников (предсказание без gold → все FP; gold без предсказания → все FN).
    let mut paths: HashSet<&str> = HashSet::new();
    paths.extend(gold_map.keys().copied());
    paths.extend(pred_map.keys().copied());

    for path in &paths {
        let empty_pred: HashSet<String> = HashSet::new();
        let pred = pred_map.get(path).copied().unwrap_or(&empty_pred);
        let gold_tags = gold_map.get(path).cloned().unwrap_or_default();
        for tag in pred.iter() {
            if !vocabulary.contains(tag) {
                out_of_vocab += 1;
                fp += 1; // вне словаря — всегда ложное срабатывание
            } else if gold_tags.contains(tag.as_str()) {
                tp += 1;
            } else {
                fp += 1;
            }
        }
        // FN — gold-теги, которых нет в предсказании.
        for g in gold_tags.iter() {
            if !pred.contains(*g) {
                fn_count += 1;
            }
        }
    }

    let precision = if tp + fp == 0 {
        0.0
    } else {
        tp as f32 / (tp + fp) as f32
    };
    let recall = if tp + fn_count == 0 {
        0.0
    } else {
        tp as f32 / (tp + fn_count) as f32
    };
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };

    ClassifyReport {
        tp,
        fp,
        fn_count,
        precision,
        recall,
        f1,
        cases: paths.len(),
        out_of_vocab,
    }
}

/// Гейт авто-тега: closed-vocab чист (`out_of_vocab == 0`) И микро-precision/recall не ниже порогов.
pub fn meets_thresholds(report: &ClassifyReport, min_precision: f32, min_recall: f32) -> bool {
    report.out_of_vocab == 0 && report.precision >= min_precision && report.recall >= min_recall
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn vocab() -> HashSet<String> {
        set(&["rust", "frontend", "ai", "design", "ops", "docs"])
    }

    /// Golden-фикстура парсится и все gold-теги — внутри словаря (инвариант набора).
    #[test]
    fn golden_parses_and_is_in_vocab() {
        let g = load_tag_golden();
        assert!(!g.cases.is_empty());
        let v: HashSet<&str> = g.vocabulary.iter().map(String::as_str).collect();
        for case in &g.cases {
            for tag in &case.gold {
                assert!(v.contains(tag.as_str()), "gold-тег {tag} вне словаря");
            }
        }
    }

    /// Идеальный классификатор (= gold) → precision=recall=1, проходит гейт.
    #[test]
    fn perfect_classifier_passes() {
        let gold = vec![
            (
                "a.md".to_string(),
                vec!["rust".to_string(), "ops".to_string()],
            ),
            ("b.md".to_string(), vec!["ai".to_string()]),
        ];
        let preds: Vec<Prediction> = gold
            .iter()
            .map(|(p, t)| (p.clone(), t.iter().cloned().collect()))
            .collect();
        let r = evaluate_tags(&preds, &gold, &vocab());
        assert_eq!(r.tp, 3);
        assert_eq!(r.fp, 0);
        assert_eq!(r.fn_count, 0);
        assert_eq!(r.precision, 1.0);
        assert_eq!(r.recall, 1.0);
        assert_eq!(r.f1, 1.0);
        assert!(meets_thresholds(&r, MIN_PRECISION, MIN_RECALL));
    }

    /// Ленивый классификатор (ничего не предсказывает) → recall=0, ГЕЙТ ПАДАЕТ (ловит регресс).
    #[test]
    fn empty_classifier_fails_recall_gate() {
        let gold = vec![(
            "a.md".to_string(),
            vec!["rust".to_string(), "ops".to_string()],
        )];
        let preds: Vec<Prediction> = vec![];
        let r = evaluate_tags(&preds, &gold, &vocab());
        assert_eq!(r.tp, 0);
        assert_eq!(r.fn_count, 2);
        assert_eq!(r.recall, 0.0);
        assert!(!meets_thresholds(&r, MIN_PRECISION, MIN_RECALL));
    }

    /// Жадный классификатор (весь словарь на каждую заметку) → precision низкий, ГЕЙТ ПАДАЕТ.
    #[test]
    fn over_eager_classifier_fails_precision_gate() {
        let gold = vec![("a.md".to_string(), vec!["rust".to_string()])];
        let preds: Vec<Prediction> = vec![(
            "a.md".to_string(),
            set(&["rust", "frontend", "ai", "design", "ops", "docs"]),
        )];
        let r = evaluate_tags(&preds, &gold, &vocab());
        assert_eq!(r.tp, 1);
        assert_eq!(r.fp, 5);
        assert!(r.precision < MIN_PRECISION);
        assert_eq!(r.recall, 1.0);
        assert!(!meets_thresholds(&r, MIN_PRECISION, MIN_RECALL));
    }

    /// Тег ВНЕ словаря → out_of_vocab>0 и считается FP; ГЕЙТ ПАДАЕТ даже при идеальном остальном.
    #[test]
    fn out_of_vocab_tag_hard_fails() {
        let gold = vec![("a.md".to_string(), vec!["rust".to_string()])];
        let preds: Vec<Prediction> = vec![("a.md".to_string(), set(&["rust", "kubernetes"]))];
        let r = evaluate_tags(&preds, &gold, &vocab());
        assert_eq!(r.tp, 1);
        assert_eq!(r.fp, 1, "kubernetes — FP");
        assert_eq!(r.out_of_vocab, 1);
        assert_eq!(r.recall, 1.0, "rust найден");
        assert!(
            !meets_thresholds(&r, MIN_PRECISION, MIN_RECALL),
            "closed-vocab нарушение валит гейт"
        );
    }

    /// Микро-агрегация: метрики суммируют TP/FP/FN по кейсам (не среднее по кейсам).
    #[test]
    fn micro_aggregation_across_cases() {
        let gold = vec![
            (
                "a.md".to_string(),
                vec!["rust".to_string(), "ops".to_string()],
            ),
            (
                "b.md".to_string(),
                vec!["ai".to_string(), "docs".to_string()],
            ),
        ];
        // a: предсказали rust(TP)+ops(TP); b: предсказали ai(TP)+design(FP), пропустили docs(FN).
        let preds: Vec<Prediction> = vec![
            ("a.md".to_string(), set(&["rust", "ops"])),
            ("b.md".to_string(), set(&["ai", "design"])),
        ];
        let r = evaluate_tags(&preds, &gold, &vocab());
        assert_eq!(r.tp, 3);
        assert_eq!(r.fp, 1);
        assert_eq!(r.fn_count, 1);
        assert_eq!(r.cases, 2);
        assert!((r.precision - 0.75).abs() < 1e-6);
        assert!((r.recall - 0.75).abs() < 1e-6);
    }

    /// Эталонный (детерминированный, БЕЗ LLM) классификатор по зашитой фикстуре: возвращает gold заметки.
    /// AI-2c заменит ЕГО на реальный `chat_util` — точка подключения гейта.
    fn reference_classify(case: &TagCase) -> HashSet<String> {
        case.gold.iter().cloned().collect()
    }

    /// END-TO-END гейт-тест (ревью EVAL-AI MAJOR): прогоняет эталон по ЗАШИТОЙ `tag_golden.json` и
    /// проверяет, что гейт ПРИМЕНЯЕТСЯ к фикстуре (а не только к инлайн-данным). Эталон проходит; пустой
    /// (ленивый) классификатор по той же фикстуре ПАДАЕТ — значит гейт дискриминирует на реальном наборе.
    #[test]
    fn fixture_runs_through_gate_and_discriminates() {
        let g = load_tag_golden();
        let vocab: HashSet<String> = g.vocabulary.iter().cloned().collect();
        let gold: Vec<(String, Vec<String>)> = g
            .cases
            .iter()
            .map(|c| (c.path.clone(), c.gold.clone()))
            .collect();

        // Эталон (= gold) проходит гейт на фикстуре.
        let good: Vec<Prediction> = g
            .cases
            .iter()
            .map(|c| (c.path.clone(), reference_classify(c)))
            .collect();
        let good_report = evaluate_tags(&good, &gold, &vocab);
        assert_eq!(good_report.out_of_vocab, 0);
        assert!(
            meets_thresholds(&good_report, MIN_PRECISION, MIN_RECALL),
            "эталон обязан проходить гейт на фикстуре"
        );

        // Ленивый (пустой) классификатор по той же фикстуре — recall=0 → гейт ПАДАЕТ.
        let lazy: Vec<Prediction> = g
            .cases
            .iter()
            .map(|c| (c.path.clone(), HashSet::new()))
            .collect();
        let lazy_report = evaluate_tags(&lazy, &gold, &vocab);
        assert!(
            !meets_thresholds(&lazy_report, MIN_PRECISION, MIN_RECALL),
            "ленивый классификатор обязан валить гейт на фикстуре"
        );
    }
}
