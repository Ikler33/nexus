//! Системное обрамление задачи СУБАГЕНТА (SUB-1) — порт hermes `_build_child_system_prompt`. Фокус:
//! ОДНА задача → вернуть КРАТКОЕ саммари. БЕЗ истории родительского разговора (изоляция контекста; в
//! SUB-3 субагент стартует с `memory=None`, так что ни recall фактов, ни история родителя не протекают).

/// Максимум символов цели/контекста в обрамлении (анти-раздувание дочернего контекста; враждебно-длинный
/// goal не должен вытеснить инструкции). UTF-8-безопасная обрезка.
const FIELD_MAX_CHARS: usize = 4000;

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Строит текст задачи субагента: цель + опц. контекст + инструкция вернуть КРАТКОЕ саммари. НЕ несёт
/// историю родителя (изоляция). Пустой/whitespace `context` → секция опускается. Поля клипуются до
/// [`FIELD_MAX_CHARS`].
pub fn build_child_task(goal: &str, context: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str(
        "Ты — сфокусированный субагент. Выполни ОДНУ задачу ниже и верни КРАТКОЕ саммари результата \
         (только итог; без рассуждений вслух, без истории).\n\n",
    );
    s.push_str("ЗАДАЧА:\n");
    s.push_str(&clip(goal, FIELD_MAX_CHARS));
    if let Some(ctx) = context {
        let ctx = clip(ctx, FIELD_MAX_CHARS);
        if !ctx.is_empty() {
            s.push_str("\n\nКОНТЕКСТ:\n");
            s.push_str(&ctx);
        }
    }
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_task_contains_goal_and_summary_instruction_and_no_parent_history() {
        let t = build_child_task("найди дубли заметок", Some("vault /notes"));
        assert!(t.contains("найди дубли заметок"), "цель включена");
        assert!(t.contains("vault /notes"), "контекст включён");
        assert!(t.contains("субагент"), "обрамление субагента");
        assert!(
            t.to_lowercase().contains("саммари"),
            "инструкция вернуть краткое саммари"
        );
        // Изоляция: нет ключевых слов «истории родителя» — обрамление само ничего не несёт.
        assert!(!t.contains("ПРЕДЫДУЩИЙ РАЗГОВОР") && !t.contains("история разговора"));
    }

    #[test]
    fn empty_context_section_omitted() {
        let none = build_child_task("цель", None);
        assert!(!none.contains("КОНТЕКСТ:"), "нет секции контекста при None");
        let blank = build_child_task("цель", Some("   "));
        assert!(
            !blank.contains("КОНТЕКСТ:"),
            "пустой контекст → секция опущена"
        );
    }

    #[test]
    fn long_fields_clipped() {
        let huge = "x".repeat(FIELD_MAX_CHARS + 500);
        let t = build_child_task(&huge, None);
        assert!(t.contains('…'), "длинная цель обрезана с маркером");
        // Грубая верхняя граница: обрамление + клипнутое поле, не весь huge.
        assert!(t.chars().count() < FIELD_MAX_CHARS + 300);
    }
}
