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

    /// SELF-LEARNING SL-7: создать/перезаписать SKILL.md агент-авторского навыка. `rel` =
    /// `<name>/SKILL.md` ОТНОСИТЕЛЬНО **skills_root** (НЕ vault — навыки живут вне note-tree, в
    /// `<vault>/.nexus/skills` по умолчанию). Файловая запись, как заметка, НО конфайнится в skills_root.
    /// classify: НИКОГДА Auto (агент не само-апрувит свои будущие инструкции), HardBlocked при выключенном
    /// `ai.skills.learning_enabled` / несконфигурированном skills_root / форме ≠ `<name>/SKILL.md` / в
    /// `vendor/`. `is_exec()==false` (запись на диск, не host-команда). **SL-7a+b (этот срез): ИНЕРТНО** —
    /// ни один инструмент не эмитит `SkillSave`, а `apply_action` fail-closed отвергает его на РУБЕЖ-0-bis
    /// (Failed, без записи). **SL-7c** проведёт реальную запись ОТДЕЛЬНЫМ путём `apply_skill_save`
    /// (skills_root-confined, обратимый snapshot) — НЕ vault-rooted `apply_action` (тот пишет в `canon_root`
    /// → для skills это PathEscape + потеря snapshot'а → тихая необратимая перезапись). v1 — только
    /// in-process (на sandbox-wire непредставим: обе `TryFrom<&Action>` границы дают `Err`).
    SkillSave { rel: String },

    // ── Фаза-3 host exec-таргеты (SANDBOX-6b) ── ИСПОЛНЯЮТСЯ ВНУТРИ песочницы после host-апрува (6c),
    // НИКОГДА не Auto, НИКОГДА не идут vault-путём apply/host-act. classify их режет HardBlocked при
    // выключенном `shell_enable` / недоступной песочнице (см. classify + §5.3). В 6b — ТОЛЬКО типы +
    // classify + fail-closed арм-ветки; рантайм-исполнение (host/exec) — 6c.
    /// Запустить shell-команду `argv` (БЕЗ оболочки) в рабочем каталоге `cwd_rel` (vault-rel) ВНУТРИ
    /// песочницы.
    ShellRun {
        argv: Vec<String>,
        cwd_rel: Option<String>,
    },
    /// Запустить процесс `program` с аргументами `args` в `cwd_rel` ВНУТРИ песочницы.
    ProcessSpawn {
        program: String,
        args: Vec<String>,
        cwd_rel: Option<String>,
    },
    /// Git-операция `op` (+`args`) над vault-репозиторием ВНУТРИ песочницы.
    GitOp { op: String, args: Vec<String> },
}

impl ActionTarget {
    /// vault-rel путь цели. У vault-вариантов — реальный путь. У exec-таргетов vault-rel НЕТ → пустой
    /// сентинел `""` (exec НЕ проходят vault-конфайнмент: classify режет их раньше по `is_exec`, а apply
    /// фейлит на top-guard'е — пустой rel не должен дойти до `resolve_vault_path_for_write`). Пустая
    /// строка к тому же fail-closed: попади она в path-конфайнмент — `EmptyPath`/HardBlock.
    pub fn rel(&self) -> &str {
        match self {
            ActionTarget::NoteCreate { rel }
            | ActionTarget::NoteEdit { rel }
            | ActionTarget::Frontmatter { rel, .. }
            // SkillSave несёт РЕАЛЬНЫЙ rel (внутри skills_root, не vault) — нужен для idempotency_key,
            // target_rel ledger'а и восстановления UndoHandle::Snapshot.rel (НЕ exec-сентинел "").
            | ActionTarget::SkillSave { rel } => rel,
            ActionTarget::ShellRun { .. }
            | ActionTarget::ProcessSpawn { .. }
            | ActionTarget::GitOp { .. } => "",
        }
    }

    /// `true` для Фаза-3 host exec-таргетов (`ShellRun`/`ProcessSpawn`/`GitOp`). Единая type-проверка:
    /// classify/dispatch/apply ветвятся по ней (exec НЕ vault-действие — не идёт apply/host-act путём).
    pub fn is_exec(&self) -> bool {
        matches!(
            self,
            ActionTarget::ShellRun { .. }
                | ActionTarget::ProcessSpawn { .. }
                | ActionTarget::GitOp { .. }
        )
    }

    /// Логическое имя инструмента (стабильный строковый дискриминант) — пишется в ledger
    /// (`agent_actions.tool_name`) и входит в [`crate::actuator::audit::idempotency_key`]. Единый
    /// источник, чтобы SQL/ключ/линты не разъехались по опечаткам.
    pub fn tool_name(&self) -> &'static str {
        match self {
            ActionTarget::NoteCreate { .. } => "note_create",
            ActionTarget::NoteEdit { .. } => "note_edit",
            ActionTarget::Frontmatter { .. } => "frontmatter",
            ActionTarget::SkillSave { .. } => "skill_save",
            ActionTarget::ShellRun { .. } => "shell_run",
            ActionTarget::ProcessSpawn { .. } => "process_spawn",
            ActionTarget::GitOp { .. } => "git_op",
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

    /// Конструктор SL-7: сохранить SKILL.md по `rel` (skills_root-rel `<name>/SKILL.md`) с телом
    /// `content` (полный текст SKILL.md: frontmatter + тело). Байт-зеркало `note_create` (content=Some,
    /// value=None). create-vs-overwrite решается по наличию файла на диске в `apply_skill_save`.
    pub fn skill_save(rel: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            target: ActionTarget::SkillSave { rel: rel.into() },
            content: Some(content.into()),
            value: None,
        }
    }

    /// Конструктор Фаза-3: shell-команда `argv` в `cwd_rel`. payload не используется (exec — не vault).
    pub fn shell_run(argv: Vec<String>, cwd_rel: Option<String>) -> Self {
        Self {
            target: ActionTarget::ShellRun { argv, cwd_rel },
            content: None,
            value: None,
        }
    }

    /// Конструктор Фаза-3: процесс `program`+`args` в `cwd_rel`.
    pub fn process_spawn(
        program: impl Into<String>,
        args: Vec<String>,
        cwd_rel: Option<String>,
    ) -> Self {
        Self {
            target: ActionTarget::ProcessSpawn {
                program: program.into(),
                args,
                cwd_rel,
            },
            content: None,
            value: None,
        }
    }

    /// Конструктор Фаза-3: git-операция `op`+`args`.
    pub fn git_op(op: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            target: ActionTarget::GitOp {
                op: op.into(),
                args,
            },
            content: None,
            value: None,
        }
    }
}
