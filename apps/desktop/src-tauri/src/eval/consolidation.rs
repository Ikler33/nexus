//! MEM-8c eval-гейт консолидации памяти (план `docs/specs/agent-memory-mem0.md` §4.5): ДЕТЕРМИНИРОВАННЫЙ
//! харнесс качества решения ADD/UPDATE/DELETE/NOOP. Главная метрика — **DELETE-precision**: доля
//! предложенных DELETE, которые ДЕЙСТВИТЕЛЬНО контрадикция (по правильной цели). Ложный DELETE = ошибочное
//! устаревание факта юзера (обратимо soft-supersede, но недопустимо в авто-режиме). Авто-DELETE (MEM-8c)
//! разблокируется ТОЛЬКО при прохождении этого гейта на ≥30 кейсах при t=0.
//!
//! Вторая метрика — **UPDATE-quality**: для кейсов UPDATE проверяем, что модель (а) выбрала UPDATE на
//! правильной цели И (б) объединённый текст СОХРАНИЛ ключевые токены (`mergeMustContain`) — прокси «не
//! теряет деталь / не галлюцинирует». Плюс **op-accuracy** (общая корректность классификации).
//!
//! По образцу `classify.rs` (EVAL-AI): фиктивный предиктор в тестах доказывает, что гейт ЛОВИТ опасный
//! регресс (триггер-хэппи DELETE) БЕЗ LLM — сам гейт тестируем. Live-точка — `live_consolidation_meets_gate`
//! (live_tests.rs) подставляет реальный `consolidate::decide` (основная модель) и сверяет с порогами.

use serde::{Deserialize, Serialize};

/// Порог DELETE-precision (безопасность авто-удаления): ≤10% ложных DELETE. Калибруется на данных;
/// высокий, т.к. ложный DELETE убирает факт юзера из ретривала.
pub const MIN_DELETE_PRECISION: f32 = 0.9;
/// Порог UPDATE-quality: ≥80% UPDATE сохраняют деталь и бьют по правильной цели.
pub const MIN_UPDATE_QUALITY: f32 = 0.8;

// `op_accuracy` и `delete_recall` НЕ в гейте — это метрики ПОЛЕЗНОСТИ, не безопасности (урок live-прогона
// gemma-26B: op_accuracy=0.784, но DELETE-precision=1.0/UPDATE-quality=1.0 — модель БЕЗОПАСНА, просто
// КОНСЕРВАТИВНА: пропускает контрадикции (missed DELETE = факты сосуществуют = НЕ потеря данных). Гейт
// разблокирует АВТО-УДАЛЕНИЕ — его критерий = ложно-удаляет ли модель (precision) и портит ли UPDATE,
// НЕ classification-accuracy, которая мешает безопасные промахи с опасными. Это именованные владельцем
// критерии (§4.5). Анти-вырожденность (precision не вакуумна) даёт `predicted_delete > 0` в гейте.

/// Один кейс консолидации: существующие факты + новый кандидат + ожидаемая операция (gold).
#[derive(Debug, Clone, Deserialize)]
pub struct ConsolidationCase {
    pub name: String,
    pub existing: Vec<String>,
    pub candidate: String,
    /// "ADD" | "UPDATE" | "DELETE" | "NOOP".
    #[serde(rename = "expectedOp")]
    pub expected_op: String,
    /// Индекс целевого факта в `existing` (для UPDATE/DELETE/NOOP). `None` для ADD.
    #[serde(default, rename = "expectedTarget")]
    pub expected_target: Option<usize>,
    /// Для UPDATE: подстроки, которые ОБЯЗАН сохранить объединённый текст (прокси «не теряет деталь»).
    #[serde(default, rename = "mergeMustContain")]
    pub merge_must_contain: Vec<String>,
    /// Для UPDATE: подстроки, которых НЕ ДОЛЖНО быть в объединённом тексте (прокси «не галлюцинирует / не
    /// инвертирует» — напр. кандидат «без сахара», merged не должен содержать «с сахаром»). M1-ревью §4.5.
    #[serde(default, rename = "mergeMustNotContain")]
    pub merge_must_not_contain: Vec<String>,
}

/// Golden-набор консолидации (`eval/consolidation_eval.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct ConsolidationGolden {
    pub cases: Vec<ConsolidationCase>,
}

/// Зашитый golden-набор консолидации.
pub fn load_consolidation_golden() -> ConsolidationGolden {
    serde_json::from_str(include_str!("../../eval/consolidation_eval.json"))
        .expect("eval/consolidation_eval.json валиден")
}

/// Предсказание модели для одного кейса: операция (UPPERCASE) + цель + объединённый текст (для UPDATE).
#[derive(Debug, Clone)]
pub struct OpPrediction {
    pub op: String,
    pub target: Option<usize>,
    pub merged: Option<String>,
}

/// Агрегированный отчёт consolidation_eval.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConsolidationReport {
    pub cases: usize,
    pub op_correct: usize,
    pub op_accuracy: f32,
    /// Сколько DELETE предложила модель.
    pub predicted_delete: usize,
    /// Из них — правильных (gold DELETE на правильной цели).
    pub correct_delete: usize,
    /// Ложных DELETE (gold НЕ delete, или удалена не та цель) — каждый = потенциальная потеря факта.
    pub false_delete: usize,
    /// Сколько DELETE в gold.
    pub gold_delete: usize,
    pub delete_precision: f32,
    pub delete_recall: f32,
    /// Сколько UPDATE-кейсов в gold.
    pub update_cases: usize,
    /// Из них — «качественных» (UPDATE на правильной цели + сохранены mergeMustContain).
    pub update_good: usize,
    pub update_quality: f32,
}

/// Считает метрики по index-выровненным `predictions` и `golden.cases`. Чистая функция (детерминизм).
/// DELETE считается верным ТОЛЬКО при gold==DELETE И совпадении цели — удаление не той цели = ложный DELETE
/// (тоже потеря данных). UPDATE «качественный» = op==UPDATE на верной цели И merged содержит ВСЕ
/// `mergeMustContain` (без учёта регистра). Пустой знаменатель → 1.0 (вакуумно безопасно: нет DELETE —
/// нет ложных удалений; нет UPDATE-кейсов — нечего терять).
pub fn evaluate_consolidation(
    predictions: &[OpPrediction],
    golden: &ConsolidationGolden,
) -> ConsolidationReport {
    let n = golden.cases.len().min(predictions.len());
    let mut op_correct = 0usize;
    let mut predicted_delete = 0usize;
    let mut correct_delete = 0usize;
    let mut false_delete = 0usize;
    let mut gold_delete = 0usize;
    let mut update_cases = 0usize;
    let mut update_good = 0usize;

    for (case, pred) in golden.cases.iter().zip(predictions.iter()) {
        let gold_op = case.expected_op.to_uppercase();
        let pred_op = pred.op.to_uppercase();

        if pred_op == gold_op {
            op_correct += 1;
        }

        if gold_op == "DELETE" {
            gold_delete += 1;
        }
        if pred_op == "DELETE" {
            predicted_delete += 1;
            if gold_op == "DELETE" && pred.target == case.expected_target {
                correct_delete += 1;
            } else {
                false_delete += 1; // удалили там, где не надо / не ту цель — потеря факта
            }
        }

        if gold_op == "UPDATE" {
            update_cases += 1;
            let target_ok = pred_op == "UPDATE" && pred.target == case.expected_target;
            let merged_lc = pred.merged.as_deref().unwrap_or("").to_lowercase();
            // Двусторонний прокси (M1-ревью): сохранены ВСЕ обязательные токены И отсутствуют запрещённые
            // (галлюцинация/инверсия). Спека §4.5: «не теряет деталь И не галлюцинирует».
            let detail_ok = case
                .merge_must_contain
                .iter()
                .all(|s| merged_lc.contains(&s.to_lowercase()))
                && case
                    .merge_must_not_contain
                    .iter()
                    .all(|s| !merged_lc.contains(&s.to_lowercase()));
            if target_ok && detail_ok {
                update_good += 1;
            }
        }
    }

    let delete_precision = if predicted_delete == 0 {
        1.0
    } else {
        correct_delete as f32 / predicted_delete as f32
    };
    let delete_recall = if gold_delete == 0 {
        1.0
    } else {
        correct_delete as f32 / gold_delete as f32
    };
    let update_quality = if update_cases == 0 {
        1.0
    } else {
        update_good as f32 / update_cases as f32
    };
    let op_accuracy = if n == 0 {
        0.0
    } else {
        op_correct as f32 / n as f32
    };

    ConsolidationReport {
        cases: n,
        op_correct,
        op_accuracy,
        predicted_delete,
        correct_delete,
        false_delete,
        gold_delete,
        delete_precision,
        delete_recall,
        update_cases,
        update_good,
        update_quality,
    }
}

/// Гейт авто-консолидации (§4.5, именованные владельцем критерии безопасности): DELETE-precision и
/// UPDATE-quality не ниже порогов, И модель предложила хотя бы один DELETE (`predicted_delete > 0` —
/// анти-вырожденность: без этого «никогда не удаляет» дало бы precision вакуумно 1.0). Прохождение
/// РАЗБЛОКИРУЕТ авто-DELETE (MEM-8c). `op_accuracy`/`delete_recall` — метрики полезности, в гейт НЕ входят
/// (см. коммент к константам): консервативная-но-безопасная модель не должна блокироваться за безопасные
/// промахи DELETE.
pub fn meets_consolidation_gate(
    report: &ConsolidationReport,
    min_delete_precision: f32,
    min_update_quality: f32,
) -> bool {
    report.predicted_delete > 0
        && report.delete_precision >= min_delete_precision
        && report.update_quality >= min_update_quality
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Предиктор = gold (идеальный): op/цель/merge точны.
    fn perfect(golden: &ConsolidationGolden) -> Vec<OpPrediction> {
        golden
            .cases
            .iter()
            .map(|c| OpPrediction {
                op: c.expected_op.clone(),
                target: c.expected_target,
                // для UPDATE подставляем текст, содержащий ВСЕ требуемые токены
                merged: if c.expected_op.eq_ignore_ascii_case("UPDATE") {
                    Some(format!("{} {}", c.existing.join(" "), c.candidate))
                } else {
                    None
                },
            })
            .collect()
    }

    #[test]
    fn golden_parses_and_is_well_formed() {
        let g = load_consolidation_golden();
        assert!(
            g.cases.len() >= 30,
            "нужно ≥30 кейсов (§4.5), сейчас {}",
            g.cases.len()
        );
        for c in &g.cases {
            let op = c.expected_op.to_uppercase();
            assert!(
                matches!(op.as_str(), "ADD" | "UPDATE" | "DELETE" | "NOOP"),
                "кейс «{}»: неизвестный op {op}",
                c.name
            );
            if op == "ADD" {
                assert!(c.expected_target.is_none(), "ADD без цели: {}", c.name);
            } else {
                let t = c.expected_target.expect("UPDATE/DELETE/NOOP нужна цель");
                assert!(t < c.existing.len(), "цель вне existing: {}", c.name);
            }
            if op == "UPDATE" {
                assert!(
                    !c.merge_must_contain.is_empty(),
                    "UPDATE нужен mergeMustContain: {}",
                    c.name
                );
            }
        }
        // Баланс набора: все четыре операции представлены.
        for op in ["ADD", "UPDATE", "DELETE", "NOOP"] {
            assert!(
                g.cases
                    .iter()
                    .any(|c| c.expected_op.eq_ignore_ascii_case(op)),
                "в наборе нет ни одного {op}"
            );
        }
        // B1-ревью: есть МУЛЬТИ-кандидатные кейсы с НЕнулевой целью — иначе выбор правильной цели
        // DELETE/UPDATE не проверяется (в проде модель видит до CONSOLIDATE_MAX_CANDIDATES=6 фактов).
        assert!(
            g.cases
                .iter()
                .any(|c| c.existing.len() >= 3 && c.expected_target.is_some_and(|t| t > 0)),
            "нет мульти-кандидатного кейса с ненулевой целью — гейт не проверяет выбор цели"
        );
    }

    #[test]
    fn perfect_predictor_passes_gate() {
        let g = load_consolidation_golden();
        let report = evaluate_consolidation(&perfect(&g), &g);
        assert_eq!(report.op_accuracy, 1.0);
        assert_eq!(report.delete_precision, 1.0);
        assert_eq!(report.update_quality, 1.0);
        assert_eq!(report.false_delete, 0);
        assert!(report.predicted_delete > 0);
        assert!(meets_consolidation_gate(
            &report,
            MIN_DELETE_PRECISION,
            MIN_UPDATE_QUALITY
        ));
    }

    /// КЛЮЧЕВОЙ тест гейта: триггер-хэппи DELETE (всё помечает DELETE цели 0) → много ложных DELETE →
    /// delete_precision рушится → гейт НЕ проходит. Доказывает, что гейт ловит ОПАСНЫЙ регресс без LLM.
    #[test]
    fn trigger_happy_delete_fails_gate() {
        let g = load_consolidation_golden();
        let preds: Vec<OpPrediction> = g
            .cases
            .iter()
            .map(|_| OpPrediction {
                op: "DELETE".into(),
                target: Some(0),
                merged: None,
            })
            .collect();
        let report = evaluate_consolidation(&preds, &g);
        assert!(report.false_delete > 0, "должны быть ложные DELETE");
        assert!(
            report.delete_precision < MIN_DELETE_PRECISION,
            "delete_precision={} должен провалить гейт",
            report.delete_precision
        );
        assert!(!meets_consolidation_gate(
            &report,
            MIN_DELETE_PRECISION,
            MIN_UPDATE_QUALITY
        ));
    }

    /// «Всегда ADD» — безопасно (нет ложных DELETE), но бесполезно: update_quality=0 → гейт не проходит.
    #[test]
    fn always_add_is_safe_but_fails_on_update_quality() {
        let g = load_consolidation_golden();
        let preds: Vec<OpPrediction> = g
            .cases
            .iter()
            .map(|_| OpPrediction {
                op: "ADD".into(),
                target: None,
                merged: None,
            })
            .collect();
        let report = evaluate_consolidation(&preds, &g);
        assert_eq!(
            report.delete_precision, 1.0,
            "нет DELETE → нет ложных удалений"
        );
        assert_eq!(report.false_delete, 0);
        assert_eq!(report.predicted_delete, 0, "ADD не предлагает DELETE");
        assert!(report.update_quality < MIN_UPDATE_QUALITY);
        assert!(!meets_consolidation_gate(
            &report,
            MIN_DELETE_PRECISION,
            MIN_UPDATE_QUALITY
        ));
    }

    /// Анти-вырожденность: модель, которая НИКОГДА не удаляет (но идеальна на остальном) → predicted_delete=0
    /// → precision вакуумно 1.0, но гейт НЕ проходит (иначе «безопасно бесполезная» разблокировала бы авто).
    #[test]
    fn never_delete_fails_anti_vacuous() {
        let g = load_consolidation_golden();
        // perfect-предсказание, но все DELETE заменены на NOOP по той же цели (никогда не удаляем).
        let preds: Vec<OpPrediction> = g
            .cases
            .iter()
            .map(|c| {
                let op = if c.expected_op.eq_ignore_ascii_case("DELETE") {
                    "NOOP".to_string()
                } else {
                    c.expected_op.clone()
                };
                OpPrediction {
                    op,
                    target: c.expected_target,
                    merged: if c.expected_op.eq_ignore_ascii_case("UPDATE") {
                        Some(format!("{} {}", c.existing.join(" "), c.candidate))
                    } else {
                        None
                    },
                }
            })
            .collect();
        let report = evaluate_consolidation(&preds, &g);
        assert_eq!(report.predicted_delete, 0);
        assert_eq!(report.delete_precision, 1.0, "вакуумная precision");
        assert_eq!(report.update_quality, 1.0);
        assert!(
            !meets_consolidation_gate(&report, MIN_DELETE_PRECISION, MIN_UPDATE_QUALITY),
            "вакуумная precision без единого DELETE не должна проходить гейт"
        );
    }

    /// UPDATE на правильной цели, но потерявший деталь (merged без требуемого токена) → не «качественный».
    #[test]
    fn update_losing_detail_is_not_quality() {
        let g = ConsolidationGolden {
            cases: vec![ConsolidationCase {
                name: "t".into(),
                existing: vec!["пьёт кофе".into()],
                candidate: "пьёт кофе по утрам".into(),
                expected_op: "UPDATE".into(),
                expected_target: Some(0),
                merge_must_contain: vec!["кофе".into(), "утр".into()],
                merge_must_not_contain: vec![],
            }],
        };
        let preds = vec![OpPrediction {
            op: "UPDATE".into(),
            target: Some(0),
            merged: Some("пьёт напитки".into()), // потеряли и «кофе», и «утр»
        }];
        let report = evaluate_consolidation(&preds, &g);
        assert_eq!(report.update_good, 0);
        assert_eq!(report.update_quality, 0.0);
    }
}
