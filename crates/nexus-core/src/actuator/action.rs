//! Типизированная алгебра действий актуатора (AGENT-3b) — fail-closed ГРАНИЦА by-construction.
//!
//! Это keystone безопасности слоя действий (ADR-009 D4): множество того, что агент МОЖЕТ предложить,
//! ограничено типом [`ActionTarget`]. Опасные классы действий — shell/process/host-команды, прямой
//! egress/сеть, запись вне vault — **намеренно отсутствуют как варианты**. Они не «проверяются в
//! рантайме и блокируются»: их попросту НЕЛЬЗЯ ВЫРАЗИТЬ. Любая попытка собрать такое действие — ошибка
//! компиляции, а не путь в [`crate::actuator::classify`], который мог бы случайно его понизить. Это и есть
//! «HardBlocked by-construction»: запрет до рантайма, без catch-all-ветки, которую можно обойти.
//!
//! Phase-C scope (этот срез) кончается на трёх vault-файловых вариантах ниже. Когда (и ЕСЛИ) появится
//! сэндбокс под shell/web (Фаза 3), новые варианты добавятся ЗДЕСЬ — и тогда `classify` ОБЯЗАН
//! получить новую ветку (exhaustive match без `_ =>` заставит компилятор это потребовать). До тех пор —
//! непредставимо ⇒ невозможно.

/// Цель действия — ЗАМКНУТОЕ множество того, что агент может предложить сделать с vault.
///
/// НЕТ вариантов shell/process/host/egress/произвольная-ФС — см. модульную доку: они HardBlocked
/// by-construction (непредставимы), а не отбраковываются в рантайме. Это инвариант, который держит
/// `classify` честным: новый вид действия физически не пройдёт мимо классификатора.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionTarget {
    /// Создать НОВУЮ заметку по vault-rel пути. По контракту цель не должна существовать (existence —
    /// забота `apply`/AGENT-3c; classify решает по пути, см. его доку).
    NoteCreate { rel: String },
    /// Перезаписать/отредактировать тело СУЩЕСТВУЮЩЕЙ заметки по vault-rel пути.
    NoteEdit { rel: String },
    /// Установить ОДИН плоский top-level frontmatter-ключ заметки. ЕДИНСТВЕННЫЙ fm-путь действия
    /// (хирургический, под snapshot-бэкап в более поздних срезах) — произвольный YAML-патч непредставим.
    Frontmatter { rel: String, key: String },
}

impl ActionTarget {
    /// vault-rel путь цели (у всех текущих вариантов он есть). Используется classify для проверки
    /// конфайнмента и ledger'ом как `target_rel`.
    pub fn rel(&self) -> &str {
        match self {
            ActionTarget::NoteCreate { rel }
            | ActionTarget::NoteEdit { rel }
            | ActionTarget::Frontmatter { rel, .. } => rel,
        }
    }

    /// Логическое имя инструмента (стабильный строковый дискриминант) — пишется в ledger
    /// (`agent_actions.tool_name`) и входит в [`crate::actuator::audit::idempotency_key`]. Единый
    /// источник, чтобы SQL/ключ/линты не разъехались по опечаткам.
    pub fn tool_name(&self) -> &'static str {
        match self {
            ActionTarget::NoteCreate { .. } => "note_create",
            ActionTarget::NoteEdit { .. } => "note_edit",
            ActionTarget::Frontmatter { .. } => "frontmatter",
        }
    }
}

/// Действие = цель + полезная нагрузка. Нагрузка типизирована по варианту цели: `content` (тело) для
/// create/edit; `value` (значение ключа) для frontmatter. Несоответствие (например, `content` у
/// Frontmatter) — не ошибка типа, а просто игнорируемое поле; нормализующие/валидирующие проверки
/// payload — в `apply` (AGENT-3c), classify решает РИСК по цели+пути, не по содержимому.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub target: ActionTarget,
    /// Тело заметки для NoteCreate/NoteEdit; `None` для Frontmatter.
    pub content: Option<String>,
    /// Значение ключа для Frontmatter; `None` для NoteCreate/NoteEdit.
    pub value: Option<String>,
}

impl Action {
    /// Конструктор create: новая заметка `rel` с телом `content`.
    pub fn note_create(rel: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            target: ActionTarget::NoteCreate { rel: rel.into() },
            content: Some(content.into()),
            value: None,
        }
    }

    /// Конструктор edit: перезапись тела заметки `rel` на `content`.
    pub fn note_edit(rel: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            target: ActionTarget::NoteEdit { rel: rel.into() },
            content: Some(content.into()),
            value: None,
        }
    }

    /// Конструктор frontmatter: установить `key`=`value` в заметке `rel`.
    pub fn frontmatter(
        rel: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            target: ActionTarget::Frontmatter {
                rel: rel.into(),
                key: key.into(),
            },
            content: None,
            value: Some(value.into()),
        }
    }
}
