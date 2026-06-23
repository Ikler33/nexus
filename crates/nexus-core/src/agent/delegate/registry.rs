//! Построение реестра инструментов СУБАГЕНТА (SUB-1) — ЧИСТАЯ логика над ИМЕНАМИ (без `ToolRegistry`/
//! спавна; реальный реестр ребёнка собирает вызывающий по этим именам в SUB-3).
//!
//! **SECURITY KEYSTONE: child ⊆ parent ВСЕГДА** (set-intersection, НИКОГДА union). Эскалация невозможна
//! ПО ПОСТРОЕНИЮ: имя, которого нет у родителя, в набор ребёнка не попадёт никаким путём. Порт hermes
//! `_build_child_agent` intersection + `DELEGATE_BLOCKED_TOOLS`.

use std::collections::BTreeSet;

/// Имя будущего инструмента делегирования (SUB-3). ВСЕГДА вырезается из реестра ребёнка → рекурсия
/// (внук) структурно невозможна (второй чекпоинт поверх depth-бюджета SUB-0).
pub const DELEGATE_RUN_TOOL: &str = "delegate.run";
/// Имя будущего инструмента deep-research (RES-*). Тоже вырезается (research сам делегирует → рекурсия).
pub const RESEARCH_RUN_TOOL: &str = "research.run";
/// Имя инструмента авторства навыков (SL-7d). Запись в ОБЩУЮ библиотеку навыков (само-обучение) —
/// прерогатива ТОП-уровня, НЕ транзиентного субагента (least-privilege; родитель по-прежнему может).
pub const SKILL_SAVE_TOOL: &str = "skill.save";

/// Инструменты, НИКОГДА не достающиеся субагенту (defense-in-depth поверх intersection):
/// - `delegate.run`/`research.run` — рекурсия/фан-аут (внук) структурно запрещены;
/// - `skill.save` — запись в общую библиотеку навыков (см. выше).
///
/// Инструмента записи в общую ПАМЯТЬ-факты в реестре агента сейчас НЕТ (память — read-only recall, не
/// tool); появится write-в-память инструмент — добавить его имя СЮДА (плановое «strip memory-write»).
const CHILD_BLOCKED_TOOLS: &[&str] = &[DELEGATE_RUN_TOOL, RESEARCH_RUN_TOOL, SKILL_SAVE_TOOL];

/// Строит набор имён инструментов для СУБАГЕНТА из имён РОДИТЕЛЯ.
///
/// Контракт (keystone): результат ⊆ `parent_names` ВСЕГДА.
/// - всегда вырезаются [`CHILD_BLOCKED_TOOLS`];
/// - `requested = Some(list)` → `list ∩ parent_names` минус блок-лист (имя НЕ у родителя — молча
///   ОТБРАСЫВАЕТСЯ, НИКОГДА не добавляется: `requested` — СУЖАЮЩИЙ фильтр, не вектор расширения);
/// - `requested = None` → `parent_names` минус блок-лист.
///
/// Возвращает ОТСОРТИРОВАННЫЙ дедуплицированный `Vec` (детерминизм).
pub fn build_child_registry(
    parent_names: &BTreeSet<String>,
    requested: Option<&[String]>,
) -> Vec<String> {
    let blocked: BTreeSet<&str> = CHILD_BLOCKED_TOOLS.iter().copied().collect();
    let allowed = |name: &str| !blocked.contains(name);
    let mut out: Vec<String> = match requested {
        // СУЖЕНИЕ: только имена, что ЕСТЬ у родителя (∩) и не в блок-листе. Неизвестное/«расширяющее»
        // имя молча отбрасывается (никогда не добавляется).
        Some(req) => req
            .iter()
            .filter(|n| parent_names.contains(n.as_str()))
            .filter(|n| allowed(n))
            .map(|n| n.to_string())
            .collect(),
        // По умолчанию ребёнок наследует ВЕСЬ инструментарий родителя минус блок-лист.
        None => parent_names
            .iter()
            .filter(|n| allowed(n))
            .cloned()
            .collect(),
    };
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent() -> BTreeSet<String> {
        [
            "note.create",
            "note.edit",
            "web.search",
            "web.fetch",
            "activate_skill",
            DELEGATE_RUN_TOOL,
            RESEARCH_RUN_TOOL,
            SKILL_SAVE_TOOL,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// KEYSTONE: что бы ни запросили (вкл. имена сверх родителя), результат ⊆ parent_names.
    #[test]
    fn child_never_exceeds_parent() {
        let p = parent();
        let requested = [
            "note.create".to_string(),
            "web.search".to_string(),
            "totally.unknown".to_string(), // нет у родителя
            "host.shell".to_string(),      // нет у родителя (попытка эскалации)
        ];
        let child = build_child_registry(&p, Some(&requested));
        assert_eq!(child, vec!["note.create", "web.search"]);
        for name in &child {
            assert!(p.contains(name), "ребёнок ⊆ родитель: {name}");
        }
    }

    /// requested-имя, которого нет у родителя, ОТБРАСЫВАЕТСЯ (не добавляется — не вектор расширения).
    #[test]
    fn requested_unknown_tool_dropped_not_added() {
        let p = parent();
        let child = build_child_registry(&p, Some(&["ghost.tool".to_string()]));
        assert!(child.is_empty(), "неизвестное имя не порождает инструмент");
    }

    /// delegate.run / research.run / skill.save вырезаны ВСЕГДА (рекурсия + запись в общую библиотеку),
    /// даже если родитель ими владеет и ребёнок их явно просит.
    #[test]
    fn blocked_tools_always_stripped_from_child() {
        let p = parent();
        // requested=None → наследование минус блок-лист.
        let inherited = build_child_registry(&p, None);
        assert!(!inherited.contains(&DELEGATE_RUN_TOOL.to_string()));
        assert!(!inherited.contains(&RESEARCH_RUN_TOOL.to_string()));
        assert!(!inherited.contains(&SKILL_SAVE_TOOL.to_string()));

        // Явный запрос блокированных — тоже не проходит.
        let requested = build_child_registry(
            &p,
            Some(&[
                DELEGATE_RUN_TOOL.to_string(),
                RESEARCH_RUN_TOOL.to_string(),
                SKILL_SAVE_TOOL.to_string(),
                "note.create".to_string(),
            ]),
        );
        assert_eq!(
            requested,
            vec!["note.create"],
            "из запроса остаётся лишь не-блокированное"
        );
    }

    /// requested=None → весь инструментарий родителя минус блок-лист (отсортировано/дедуп).
    #[test]
    fn none_request_inherits_parent_minus_blocked() {
        let p = parent();
        let child = build_child_registry(&p, None);
        assert_eq!(
            child,
            vec![
                "activate_skill",
                "note.create",
                "note.edit",
                "web.fetch",
                "web.search",
            ],
            "наследовано всё кроме delegate.run/research.run/skill.save"
        );
    }

    /// Пустой parent → пустой ребёнок при любом запросе (нечего сужать).
    #[test]
    fn empty_parent_yields_empty_child() {
        let empty = BTreeSet::new();
        assert!(build_child_registry(&empty, None).is_empty());
        assert!(build_child_registry(&empty, Some(&["note.create".to_string()])).is_empty());
    }
}
