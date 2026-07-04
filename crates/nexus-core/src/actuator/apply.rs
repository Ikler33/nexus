//! APPLY-исполнитель актуатора (AGENT-3c, Фаза 1) — host-side запись в vault ЗА ВСЕМИ рубежами.
//!
//! Здесь — и ТОЛЬКО здесь — действие актуатора реально касается диска. [`apply_action`] исполняет одно
//! классифицированное действие в СТРОГОМ порядке рубежей (нарушить порядок — потерять безопасность):
//!
//! ```text
//!  1. canonicalize/symlink RAMPART  resolve_vault_path_for_write(canon_root, rel)  ← 2-й рубеж к classify
//!  2. existence / read-current      NoteCreate: цели НЕ должно быть; Edit/Frontmatter: читаем диск
//!  3. optimistic-concurrency        on-disk content_hash NOW vs classify_hash → drift ⇒ Failed (не клобберим)
//!  4. ledger WRITE-BEFORE-ACT       record_before(state=Executing, outcome=NULL)  ← ДО любого write
//!     ├─ дубль ключа (UNIQUE)       → replay_decision: AlreadyDone⇒return; Crashed⇒re-check; Fresh⇒дальше
//!  5. snapshot-before (manual=true) для overwrite: snapshot(canon_root, rel, current, /*manual=*/true)
//!     ⇒ UndoHandle::Snapshot{rel, ts}   |  NoteCreate ⇒ UndoHandle::Trash{trash_rel}
//!  5b.re-read fence (overwrite)     перечитать диск, hash == hash(current_content)? нет ⇒ Failed(drift)
//!  6. WRITE                         atomic_write(abs, …)  |  set_frontmatter_field → atomic_write
//!  7. ledger FINISH                 finish(key, Executed, outcome, Some(undo))  (на ошибке: Failed, None)
//! ```
//!
//! ## Два рубежа конфайнмента (keystone безопасности)
//! `classify` (3b) делает ЛЕКСИЧЕСКУЮ проверку пути — она НЕ видит симлинк ВНУТРИ vault, ведущий НАРУЖУ
//! (лексически `evil.md` — обычное имя в корне → Auto). [`apply_action`] ОБЯЗАН писать ТОЛЬКО по пути,
//! который вернул [`resolve_vault_path_for_write`] (он канонизирует РОДИТЕЛЯ и проверяет
//! `starts_with(canon_root)` — следует по симлинку-КАТАЛОГУ и ловит побег). НИКОГДА не писать в
//! `canon_root.join(rel)` напрямую. resolve канонизирует РОДИТЕЛЯ, но НЕ сам leaf — поэтому рубеж 1
//! ДОПОЛНИТЕЛЬНО отвергает leaf-СИМЛИНК (`symlink_metadata` без следования): `vault/evil.md ->
//! /outside/secret.md` имеет родителя-внутри, но сам — симлинк наружу; без этой проверки чтение утекло бы
//! внешним содержимым в снапшот. Любой Err рубежа 1 ⇒ НИ ОДНОГО write/чтения ([`ApplyOutcome::PathEscape`]).
//!
//! Рубеж 1 несёт ещё две fail-closed проверки (review hard-gates): (Fix 1) ДО `create_dir_all` для create
//! проходим компоненты родителя от canon_root вниз — любой предсуществующий компонент-СИМЛИНК ⇒ PathEscape
//! (иначе create_dir_all создал бы пустые каталоги ВНЕ vault сквозь симлинк-каталог до конфайнмент-чека);
//! (Fix 3, cfg(unix)) для overwrite сверяем `nlink>1` — ХАРДЛИНК на внешний inode (симлинком НЕ детектится)
//! утёк бы внешним содержимым в снапшот ⇒ PathEscape.
//!
//! ## Ledger write-before-act = фенс идемпотентности
//! Строка в `agent_actions` (state=Executing, outcome=NULL) пишется ДО любого касания диска. UNIQUE
//! `idempotency_key` — фенс: повтор того же действия отбивается, и [`AuditSink::replay_decision`] решает по
//! ПРИСУТСТВИЮ outcome (AlreadyDone ⇒ НЕ переписываем; CrashedMidExecute ⇒ re-check on-disk hash;
//! Fresh ⇒ исполняем). Так краш МЕЖДУ ledger-строкой и write восстановим: строка-якорь уже есть.
//!
//! ## Граница 3c/3d (НЕ здесь)
//! Тир Confirm здесь НЕ исполняется и НЕ пишет: [`apply_action`] вызывается инструментом ТОЛЬКО для Auto
//! (Confirm → proposed-not-applied на уровне tools.rs). Enforcement автономии (confirm|auto run-level),
//! DecisionSource, эмиссия Proposal/Diff, blast-radius — AGENT-3d. Проводки в agentd/registry нет (3e).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::db::{DbResult, ReadPool, WriteActor};

use super::action::{Action, ActionTarget};
use super::audit::{
    self, canonical_args, idempotency_key, ActionEntry, ReplayDecision, STATE_EXECUTING,
    STATE_FAILED,
};
use super::UndoHandle;

/// Хэндл к idempotency-ledger (`agent_actions`) для apply — тонкая обёртка над аудированными свободными
/// функциями [`super::audit`] (record_before/finish/replay_decision/lookup). Держит клон-хэндлы writer'а
/// и reader'а БД (оба `Clone`, ADR-003). Существует, чтобы apply/tools звали `ledger.record_before(…)` /
/// `ledger.finish(…)` единым объектом (брифовый `ledger: &AuditSink`), не таская порознь writer+reader.
#[derive(Clone)]
pub struct AuditSink {
    writer: WriteActor,
    reader: ReadPool,
}

impl AuditSink {
    /// Сконструировать sink из клон-хэндлов БД (writer для мутаций, reader для replay-чтений).
    pub fn new(writer: WriteActor, reader: ReadPool) -> Self {
        Self { writer, reader }
    }

    /// Write-before-act: вставить строку действия (outcome=NULL) ДО эффекта. UNIQUE-фенс на дубль —
    /// `Err` (caller тогда делает [`AuditSink::replay_decision`]). См. [`audit::record_before`].
    pub async fn record_before(&self, entry: ActionEntry) -> DbResult<i64> {
        audit::record_before(&self.writer, entry).await
    }

    /// Терминировать действие: state+outcome (+опц. undo), поглощающе. См. [`audit::finish`].
    pub async fn finish(
        &self,
        key: &str,
        state: &str,
        outcome: &str,
        undo: Option<super::audit::UndoCols>,
    ) -> DbResult<bool> {
        audit::finish(&self.writer, key, state, outcome, undo).await
    }

    /// Replay-решение по ключу — ветвление по ПРИСУТСТВИЮ outcome. См. [`audit::replay_decision`].
    pub async fn replay_decision(&self, key: &str) -> DbResult<ReplayDecision> {
        audit::replay_decision(&self.reader, key).await
    }

    /// Клон writer-хэндла (ADR-003 — дёшев, актор сериализует мутации). Нужен гейту автономии
    /// ([`super::orchestrate`]) для `proposed→approved` ([`audit::transition`]) — переход состояния
    /// предложения вне набора методов sink'а. Не расширяет инварианты: те же сериализованные мутации.
    pub fn writer_handle(&self) -> WriteActor {
        self.writer.clone()
    }

    /// Клон reader-хэндла. Нужен гейту автономии для lookup строки предложения (идемпотентность
    /// предложения / проверки ledger в тестах).
    pub fn reader_handle(&self) -> ReadPool {
        self.reader.clone()
    }
}

/// Исход исполнения одного действия [`apply_action`]. Несёт ровно то, что нужно вызывающему (tools.rs):
/// человеко-читаемое резюме + UndoHandle (для Executed). Все варианты, кроме [`ApplyOutcome::Executed`],
/// означают «диск НЕ изменён» (fail-closed — мы либо записали и отдали корректный undo, либо не трогали
/// файл вовсе).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Запись прошла. `summary` — резюме для tool-результата; `undo` — КОРРЕКТНЫЙ хэндл отката
    /// (Snapshot{ts} для overwrite, Trash{trash_rel} для create). Никогда не отдаётся undo, который
    /// нельзя исполнить.
    Executed { summary: String, undo: UndoHandle },
    /// Действие УЖЕ было исполнено в этом прогоне (replay AlreadyDone) — диск НЕ трогали повторно;
    /// несём записанный ранее outcome. Идемпотентность: тот же эффект не наносится дважды.
    AlreadyDone(String),
    /// Рубеж canonicalize/symlink (resolve_vault_path_for_write) отверг путь — НИ ОДНОГО write.
    /// Это ловит симлинк ВНУТРИ vault наружу, который лексический classify пропустил как Auto.
    PathEscape,
    /// Действие не исполнено по безопасной причине, диск НЕ изменён. `reason` — пояснение
    /// (цель create уже существует; drift оптимистичной конкуренции; ошибка записи/ledger и т.п.).
    Failed(String),
}

/// Причина дрейфа/несоответствия для читаемого Failed-сообщения.
fn drift_msg(rel: &str) -> String {
    format!(
        "конкурентный дрейф: содержимое {rel} изменилось с момента классификации — запись отменена \
         (перечитайте и повторите)"
    )
}

/// Исполнить одно классифицированное действие в vault за всеми рубежами (см. модульную доку — порядок
/// рубежей строгий). `canon_root` ОБЯЗАН быть уже канонизированным корнем vault (предусловие
/// [`resolve_vault_path_for_write`]). `run_id` — прогон (для ledger-корреляции и idempotency_key).
/// `classify_hash` — on-disk content_hash цели, снятый НА МОМЕНТ classify (токен оптимистичной
/// конкуренции; `None` — проверку дрейфа пропускаем). `classify_hash` ОТДЕЛЕН от target_hash в ключе:
/// здесь он — токен re-check, а в ключ идёт отпечаток цели (см. ниже).
///
/// Вызывается ТОЛЬКО для Auto-тира (Confirm → proposed-not-applied в tools.rs; HardBlocked → ToolError
/// ещё до apply). Любой исход, кроме [`ApplyOutcome::Executed`], гарантирует «диск не изменён».
///
/// ## Допущение единственного писателя (Fix 4)
/// Гарантии обратимости/TOCTOU исходят из того, что агент — ЕДИНСТВЕННЫЙ писатель цели в окне read→write
/// одного прогона. Многописательские фенсы (внешняя правка/синк/git/другой редактор в этом окне) — это
/// drift-проверка Рубежа 3 (по `classify_hash`) ПЛЮС безусловный ре-рид прямо перед записью (Fix 2,
/// overwrite-путь, независим от classify_hash). 3d ОБЯЗАН передавать `classify_hash` на пути живого
/// changeset, чтобы дрейф ловился и до снапшота (Рубеж 3), а не только ре-ридом перед записью.
///
/// ## No-bypass — КОМПАЙЛ-ТАЙМ (AGENT-3e Fix-2)
/// Видимость СУЖЕНА до `pub(in crate::actuator)`: вызвать `apply_action` может ТОЛЬКО код внутри
/// модуля `actuator` — на практике это [`super::orchestrate::apply_now`] (единственный не-тестовый
/// вызыватель) и юнит-тесты этого файла. Любой будущий инструмент/модуль ВНЕ `crate::actuator`,
/// попытавшийся применить действие напрямую в обход гейта автономии
/// ([`super::orchestrate::dispatch_action`]), НЕ СКОМПИЛИРУЕТСЯ (E0603). Так no-bypass-гарантия (3e
/// hard-gate #1) держится компилятором, а не конвенцией. НЕ расширять видимость обратно до `pub` —
/// это вернёт ungated direct-apply путь, который go-live-ревью требует закрыть.
pub(in crate::actuator) async fn apply_action(
    action: &Action,
    run_id: i64,
    canon_root: &Path,
    ledger: &AuditSink,
    classify_hash: Option<&str>,
) -> ApplyOutcome {
    // ── РУБЕЖ 0 (CHOKEPOINT): exec-таргет НЕ пишется vault-путём (host/exec — Фаза-3 6c); fail-closed.
    if action.target.is_exec() {
        return ApplyOutcome::Failed(
            "exec-таргет не применяется vault-путём apply (host/exec — Фаза-3 6c)".into(),
        );
    }
    // ── РУБЕЖ 0-bis (SL-7): SkillSave — ОТДЕЛЬНЫЙ путь apply_skill_save (skills_root); fail-closed.
    if matches!(action.target, ActionTarget::SkillSave { .. }) {
        return ApplyOutcome::Failed(
            "SkillSave не применяется vault-путём apply (skills_root — отдельный путь apply_skill_save)"
                .into(),
        );
    }
    // РУБЕЖ 1 — конфайнмент; рубежи 2-7 (строгий порядок) — ядро apply_confined_write (NOTE_SPEC).
    let rel = action.target.rel().to_string();
    let is_create = matches!(action.target, ActionTarget::NoteCreate { .. });
    let abs = match resolve_note_path(canon_root, &rel, is_create).await {
        Ok(abs) => abs,
        Err(outcome) => return outcome,
    };
    apply_confined_write(
        action,
        run_id,
        canon_root,
        abs,
        ledger,
        classify_hash,
        NOTE_SPEC,
    )
    .await
}

/// SELF-LEARNING SL-7c: применить `SkillSave` — атомарная ОБРАТИМАЯ запись `<name>/SKILL.md` под
/// **skills_root** (НЕ vault `canon_root`). Зеркало [`apply_action`]-рубежей, но: (1) корень — skills_root;
/// (2) create-vs-overwrite ДАННО-ОПРЕДЕЛЯЕТСЯ наличием файла (у `SkillSave` нет отдельных Create/Edit —
/// перезапись СВОЕГО навыка легитимна; запрет перезаписи ЧУЖОГО — на tool-слое SL-7d по `created_by`);
/// (3) содержимое РАУНД-ТРИПИТСЯ через [`crate::skills::parse_skill`] ПЕРЕД записью (битый frontmatter →
/// Failed, диск не тронут — навык обязан быть перезагружаемым). Обратимость: create→Trash, overwrite→
/// Snapshot (history под skills_root) — ДО записи; снапшот не записался ⇒ Failed без записи (fail-closed).
/// `pub(in crate::actuator)` — вызывается ТОЛЬКО из `dispatch_skill_save` (нет ungated-пути), как
/// `apply_action`. classify-гейт (Confirm-never-Auto + skills_root/vendor/форма) — у вызывателя.
// Прод-путь (SL-7d): `dispatch_skill_save` ← `SkillSaveCtx::apply` ← зарегистрированный `SkillSaveTool`.
pub(in crate::actuator) async fn apply_skill_save(
    action: &Action,
    run_id: i64,
    skills_root: &Path,
    ledger: &AuditSink,
    classify_hash: Option<&str>,
    agent_paused: &Arc<AtomicBool>,
) -> ApplyOutcome {
    // Пред-фенсовые defense-in-depth guard'ы (SkillSave/kill-switch/лексический конфайн/round-trip) — ДО
    // рубежа 1 (kill-switch ДО любого ledger/disk-эффекта). rel/content — чистые вычисления, побочек нет.
    let rel = action.target.rel().to_string();
    let content = action.content.clone().unwrap_or_default();
    if let Err(outcome) = skill_pre_checks(action, agent_paused, &rel, &content) {
        return outcome;
    }
    // РУБЕЖ 1 — конфайнмент в skills_root; рубежи 2-7 — ядро apply_confined_write (SKILL_SPEC).
    let abs = match resolve_skill_path(skills_root, &rel).await {
        Ok(abs) => abs,
        Err(outcome) => return outcome,
    };
    apply_confined_write(
        action,
        run_id,
        skills_root,
        abs,
        ledger,
        classify_hash,
        SKILL_SPEC,
    )
    .await
}

/// Пред-фенсовые defense-in-depth-проверки навыка (ДО рубежей ядра), в исходном порядке: (1) таргет —
/// SkillSave (dispatch_skill_save зовёт только с ним); (2) KILL-SWITCH (AGENT-5) — читаем ДО любого
/// ledger/disk-эффекта, под паузой НИ ОДНОЙ записи (Failed, строки-якоря нет, retry-safe); (3) ЛЕКСИЧЕСКИЙ
/// конфайн пути (надмножество-фильтр classify, ДО create_dir_all рубежа 1 — иначе `../x` создал бы каталог
/// ВНЕ skills_root); (4) round-trip навыка через `parse_skill` (битый frontmatter → Failed, БЕЗ записи —
/// навык обязан быть перезагружаемым). `Err` ⇒ готовый [`ApplyOutcome`] для немедленного return.
fn skill_pre_checks(
    action: &Action,
    agent_paused: &Arc<AtomicBool>,
    rel: &str,
    content: &str,
) -> Result<(), ApplyOutcome> {
    if !matches!(action.target, ActionTarget::SkillSave { .. }) {
        return Err(ApplyOutcome::Failed(
            "apply_skill_save вызван не для SkillSave".into(),
        ));
    }
    if agent_paused.load(Ordering::Relaxed) {
        return Err(ApplyOutcome::Failed(
            "агент на паузе (kill-switch) — запись навыка подавлена".into(),
        ));
    }
    if super::classify::path_confinement(rel).is_err() {
        return Err(ApplyOutcome::PathEscape);
    }
    if let Err(e) = crate::skills::parse_skill(content, rel) {
        return Err(ApplyOutcome::Failed(format!(
            "SkillSave: SKILL.md не парсится ({e}) — не записан"
        )));
    }
    Ok(())
}

/// РУБЕЖ 1 для vault-заметки (canonicalize/symlink RAMPART — 2-й рубеж к лексическому classify). Пишем
/// ТОЛЬКО по возвращённому `abs` (НИКОГДА по `canon_root.join(rel)`). create-путь: symlink-безопасный
/// create_dir_all (Fix 1 — [`reject_symlinked_components`] ДО create_dir_all, иначе создались бы каталоги
/// ВНЕ vault сквозь предсуществующий симлинк-каталог) + [`resolve_vault_path_for_write`] + leaf-симлинк
/// reject; overwrite-путь: [`confine_for_overwrite`] (resolve + leaf-симлинк + хардлинк). `Err` ⇒ готовый
/// [`ApplyOutcome`] (PathEscape / Failed(join)) для немедленного `return` — НИ ОДНОГО write/чтения.
async fn resolve_note_path(
    canon_root: &Path,
    rel: &str,
    is_create: bool,
) -> Result<PathBuf, ApplyOutcome> {
    let canon_root = canon_root.to_path_buf();
    let rel_path = PathBuf::from(rel);
    let resolved = tokio::task::spawn_blocking(move || {
        if is_create {
            // resolve канонизирует НЕПОСРЕДСТВЕННОГО родителя — он ОБЯЗАН существовать. Для create в новую
            // подпапку создаём parent ДО резолва: СНАЧАЛА reject_symlinked_components (симлинк-компонент
            // наружу ⇒ PathEscape ДО create_dir_all), потом create_dir_all свежих (не-симлинк) каталогов.
            if let Some(parent) = canon_root.join(&rel_path).parent() {
                if !parent.exists() {
                    reject_symlinked_components(&canon_root, &rel_path)?;
                    std::fs::create_dir_all(parent)?;
                }
                let abs = crate::vault::resolve_vault_path_for_write(&canon_root, &rel_path)?;
                // leaf-симлинк-пустышка с тем же именем (родитель внутри, leaf наружу) ⇒ PathEscape.
                if let Ok(meta) = std::fs::symlink_metadata(&abs) {
                    if meta.file_type().is_symlink() {
                        return Err(crate::vault::VaultError::PathEscape);
                    }
                }
                return Ok::<_, crate::vault::VaultError>(abs);
            }
        }
        confine_for_overwrite(&canon_root, &rel_path)
    })
    .await;
    match resolved {
        Ok(Ok(abs)) => Ok(abs),
        Ok(Err(_)) => Err(ApplyOutcome::PathEscape),
        Err(join_err) => Err(ApplyOutcome::Failed(format!("apply join: {join_err}"))),
    }
}

/// РУБЕЖ 1 для навыка (конфайнмент в **skills_root**): symlink-безопасный create_dir_all(parent) +
/// [`confine_for_overwrite`] (resolve + leaf-симлинк + хардлинк; для нового leaf'а симлинк/хардлинк-проверки
/// просто проходят — файла нет). `Err` ⇒ готовый [`ApplyOutcome`] (PathEscape / Failed(join)) для `return`.
async fn resolve_skill_path(skills_root: &Path, rel: &str) -> Result<PathBuf, ApplyOutcome> {
    let root = skills_root.to_path_buf();
    let rel_path = PathBuf::from(rel);
    let resolved = tokio::task::spawn_blocking(move || {
        if let Some(parent) = root.join(&rel_path).parent() {
            if !parent.exists() {
                reject_symlinked_components(&root, &rel_path)?;
                std::fs::create_dir_all(parent)?;
            }
        }
        confine_for_overwrite(&root, &rel_path)
    })
    .await;
    match resolved {
        Ok(Ok(abs)) => Ok(abs),
        Ok(Err(_)) => Err(ApplyOutcome::PathEscape),
        Err(join_err) => Err(ApplyOutcome::Failed(format!(
            "skill apply join: {join_err}"
        ))),
    }
}

/// Провал снапшота Рубежа 5 (для различного у заметки/навыка текста): точка не записалась / ошибка
/// snapshot / join-ошибка блокирующей задачи. Несёт заимствования, чтобы hook форматировал сообщение.
enum SnapFail<'e> {
    NotWritten,
    Failed(&'e crate::vault::VaultError),
    Join(&'e tokio::task::JoinError),
}

/// Спека, ЧЕМ различаются два вызывателя [`apply_confined_write`] (vault-заметка vs навык). ВСЁ, что
/// отличается между двумя путями записи, — здесь; рубежи 2-7 (их ПОРЯДОК — единственный источник истины)
/// остаются единым телом ядра. Хуки — свободные `fn` (без захвата), поэтому [`NOTE_SPEC`]/[`SKILL_SPEC`]
/// — `const`; `Copy` (все поля — `fn`-указатели/`&'static str`), передаётся в ядро по значению.
/// Различия НЕ размазаны обратно по телу: ядро видит только `spec`.
#[derive(Clone, Copy)]
struct ConfinedWriteSpec {
    /// create-vs-overwrite: заметка — по типу таргета (NoteCreate); навык — по отсутствию файла на диске
    /// (перезапись СВОЕГО навыка легитимна). Аргумент bool — `current_content.is_none()`.
    is_create: fn(&Action, bool) -> bool,
    /// Existence-вердикт Рубежа 2 (только для Fresh): заметка ловит create-над-существующим /
    /// edit-над-отсутствующим (`Some(msg)`); навык — всегда `None` (create-vs-overwrite данно-определён).
    /// Аргументы: action, is_create, current_content.is_some(), rel.
    existence_verdict: fn(&Action, bool, bool, &str) -> Option<String>,
    /// Сообщение, когда INSERT строки-якоря ledger (Рубеж 4) упал НЕ по UNIQUE-дублю (ветка Fresh).
    ledger_write_fail: &'static str,
    /// Сообщение провала снапшота (Рубеж 5) — тексты у заметки и навыка различны.
    snapshot_fail_msg: fn(&str, SnapFail<'_>) -> String,
    /// WRITE (Рубеж 6): байты записи (или строка-ошибка) из action + прочитанного содержимого. Ядро само
    /// делает [`spawn_atomic_write`] по `abs` — единственная точка касания диска в конвейере.
    build_write: fn(&Action, &str) -> Result<Vec<u8>, String>,
    /// Резюме успеха (Рубеж 7) для tool-результата/ledger-outcome. Аргументы: action, rel, is_create.
    success_summary: fn(&Action, &str, bool) -> String,
}

fn note_is_create(action: &Action, _current_none: bool) -> bool {
    matches!(action.target, ActionTarget::NoteCreate { .. })
}

fn skill_is_create(_action: &Action, current_none: bool) -> bool {
    current_none
}

fn note_existence_verdict(
    action: &Action,
    is_create: bool,
    current_is_some: bool,
    rel: &str,
) -> Option<String> {
    if is_create && current_is_some {
        Some(format!(
            "note.create: цель {rel} уже существует — создание отменено (используйте edit для правки)"
        ))
    } else if !is_create && !current_is_some {
        Some(format!(
            "{}: цель {rel} не существует — править нечего",
            action.target.tool_name()
        ))
    } else {
        None
    }
}

fn no_existence_verdict(_a: &Action, _is_create: bool, _some: bool, _rel: &str) -> Option<String> {
    None
}

fn note_snapshot_fail(rel: &str, fail: SnapFail<'_>) -> String {
    match fail {
        SnapFail::NotWritten => format!(
            "snapshot-before {rel}: точка восстановления не создана — запись отменена \
             (обратимость не гарантирована)"
        ),
        SnapFail::Failed(e) => format!("snapshot-before {rel}: {e}"),
        SnapFail::Join(e) => format!("snapshot join: {e}"),
    }
}

fn skill_snapshot_fail(rel: &str, fail: SnapFail<'_>) -> String {
    match fail {
        SnapFail::NotWritten => {
            format!("SkillSave: снапшот пред-правки {rel} не записан — перезапись отменена")
        }
        SnapFail::Failed(e) => {
            format!("SkillSave: снапшот пред-правки {rel} провалился ({e}) — отмена")
        }
        SnapFail::Join(e) => format!("SkillSave snapshot join: {e}"),
    }
}

fn note_build_write(action: &Action, current_content: &str) -> Result<Vec<u8>, String> {
    match &action.target {
        ActionTarget::NoteCreate { .. } | ActionTarget::NoteEdit { .. } => {
            Ok(action.content.clone().unwrap_or_default().into_bytes())
        }
        ActionTarget::Frontmatter { key: fm_key, .. } => {
            // read → set_frontmatter_field → atomic_write. База — СВЕЖИЙ on-disk (current_content);
            // round-trip-reject set_frontmatter_field — единственный санкционированный fm-писатель.
            let value = action.value.clone().unwrap_or_default();
            match crate::parser::set_frontmatter_field(current_content, fm_key, &value) {
                Ok(new_content) => Ok(new_content.into_bytes()),
                // AGENT-6 privacy: `FmWriteError` — только unit-варианты, `{e:?}` печатает лишь ИМЯ варианта.
                Err(e) => Err(format!("set_frontmatter_field: {e:?}")),
            }
        }
        // ПРОВАБЛИ-МЁРТВО: exec отсечён top-guard'ом РУБЕЖА 0 в apply_action (сюда не доходит).
        ActionTarget::ShellRun { .. }
        | ActionTarget::ProcessSpawn { .. }
        | ActionTarget::GitOp { .. } => {
            unreachable!("exec-таргет отсечён top-guard РУБЕЖА 0 в apply_action")
        }
        // SL-7: SkillSave отсечён top-guard'ом РУБЕЖА 0-bis (идёт через apply_skill_save).
        ActionTarget::SkillSave { .. } => {
            unreachable!("SkillSave отсечён top-guard РУБЕЖА 0-bis в apply_action")
        }
    }
}

fn skill_build_write(action: &Action, _current_content: &str) -> Result<Vec<u8>, String> {
    Ok(action.content.clone().unwrap_or_default().into_bytes())
}

fn note_success_summary(action: &Action, rel: &str, _is_create: bool) -> String {
    success_summary(action, rel)
}

fn skill_success_summary(_action: &Action, rel: &str, is_create: bool) -> String {
    let verb = if is_create {
        "создан"
    } else {
        "обновлён"
    };
    format!("навык {verb}: {rel}")
}

/// Спека vault-заметки: canon_root; create-vs-overwrite по типу таргета; existence-вердикт активен.
const NOTE_SPEC: ConfinedWriteSpec = ConfinedWriteSpec {
    is_create: note_is_create,
    existence_verdict: note_existence_verdict,
    ledger_write_fail: "ledger record_before: запись строки действия не удалась",
    snapshot_fail_msg: note_snapshot_fail,
    build_write: note_build_write,
    success_summary: note_success_summary,
};

/// Спека навыка: skills_root; create-vs-overwrite по наличию файла; existence-вердикта НЕТ.
const SKILL_SPEC: ConfinedWriteSpec = ConfinedWriteSpec {
    is_create: skill_is_create,
    existence_verdict: no_existence_verdict,
    ledger_write_fail: "ledger record_before: запись строки SkillSave не удалась",
    snapshot_fail_msg: skill_snapshot_fail,
    build_write: skill_build_write,
    success_summary: skill_success_summary,
};

/// ЯДРО безопасной записи в конфайнед-корень: рубежи 2-7 в СТРОГОМ, ЕДИНСТВЕННОМ порядке (нарушить
/// порядок — потерять безопасность). Общее тело для [`apply_action`] (vault) и [`apply_skill_save`]
/// (навык); вызыватель уже прошёл рубежи 0/0-bis/1 и передаёт резолвнутый `abs` (Рубеж 1) + `spec`
/// различий (`root` для снапшота, create-политика, existence-вердикт, тексты, WRITE, резюме). Порядок:
/// ```text
///   2.   read-current → target_hash / idempotency_key
///   3.   optimistic-concurrency drift (classify_hash) — ДО фенса
///   4.   ledger WRITE-BEFORE-ACT (+ replay: AlreadyDone / Crashed re-check / Fresh)
///   [2.] existence-вердикт (ТОЛЬКО Fresh; spec-зависим)
///   5.   snapshot-before-act (undo: Trash | Snapshot) — fail-closed
///   F2.  безусловный re-read fence (overwrite) — не клобберим внешнюю правку
///   6.   atomic WRITE по `abs` (НИКОГДА по root.join(rel))
///   7.   ledger FINISH (поглощающий)
/// ```
/// Любой исход, кроме [`ApplyOutcome::Executed`], гарантирует «диск не изменён» (либо записан с корректным
/// undo). Атомарность (atomic_write rename) и порядок рубежей — инвариант; менять их НЕЛЬЗЯ.
async fn apply_confined_write(
    action: &Action,
    run_id: i64,
    root: &Path,
    abs: PathBuf,
    ledger: &AuditSink,
    classify_hash: Option<&str>,
    spec: ConfinedWriteSpec,
) -> ApplyOutcome {
    let rel = action.target.rel().to_string();

    // ── РУБЕЖ 2: read-current (для hash/снапшота). Existence-ВЕРДИКТ выносим ПОСЛЕ ledger-фенса: replay
    // того же create (файл уже создан 1-м прогоном) должен дать AlreadyDone, а не «уже существует» —
    // идемпотентность сильнее существования. read — это просто чтение (не вердикт), его делаем сейчас.
    let current_content: Option<String> = read_to_string_opt(&abs).await;
    let on_disk_hash: Option<String> = current_content
        .as_ref()
        .map(|c| crate::vault::content_hash(c.as_bytes()));
    let is_create = (spec.is_create)(action, current_content.is_none());

    // Идемпотентность-ключ: target_hash — отпечаток цели НА МОМЕНТ classify (часть тождества действия,
    // СТАБИЛЕН краш→retry). classify_hash задан ⇒ берём его (at-classify view); None ⇒ create: отпечаток
    // ПЛАНИРУЕМОГО содержимого, overwrite: on-disk сейчас. content_hash-колонка ledger — ОТДЕЛЬНЫЙ токен
    // оптимистичной конкуренции (on-disk-at-attempt для Crashed re-check); он НЕ часть ключа.
    let payload = action_payload(action);
    let target_hash = match classify_hash {
        Some(h) => h.to_string(),
        None => {
            if is_create {
                crate::vault::content_hash(payload.unwrap_or("").as_bytes())
            } else {
                on_disk_hash
                    .clone()
                    .unwrap_or_else(|| crate::vault::content_hash(payload.unwrap_or("").as_bytes()))
            }
        }
    };
    let args = canonical_args(Some(&rel), payload);
    let key = idempotency_key(run_id, action.target.tool_name(), &args, &target_hash);

    // ── РУБЕЖ 3: optimistic concurrency (classify_hash drift) — ДО фенса (stale view → не клобберим).
    // on-disk hash СЕЙЧАС vs classify_hash. None on_disk (create) → "" (цели нет).
    if let Some(expected) = classify_hash {
        let now = on_disk_hash.as_deref().unwrap_or("");
        if now != expected {
            return ApplyOutcome::Failed(drift_msg(&rel));
        }
    }

    // ── РУБЕЖ 4: ledger WRITE-BEFORE-ACT (ДО любого write). INSERT строки-якоря (outcome=NULL) — ФЕНС.
    // Успех ⇒ Fresh. Дубль (UNIQUE) ⇒ replay того же действия → ветвимся по присутствию outcome.
    // ПРИВАТНОСТЬ (AGENT-6): долговечное diff_summary — СТРУКТУРНОЕ (счётчики строк + статус-токен),
    // свободное от содержимого; строит его редакция-гвард diff_summary_for. Ни тело, ни значения — не в журнал.
    let diff_summary =
        super::orchestrate::diff_summary_for(action, current_content.as_deref().unwrap_or(""))
            .render();
    let entry = ActionEntry {
        run_id,
        idempotency_key: key.clone(),
        tool_name: action.target.tool_name().to_string(),
        target_rel: Some(rel.clone()),
        risk_tier: audit::TIER_AUTO.to_string(),
        state: STATE_EXECUTING.to_string(),
        content_hash: on_disk_hash.clone(),
        diff_summary: Some(diff_summary),
    };
    let is_fresh = match ledger.record_before(entry).await {
        Ok(_) => true, // Fresh: строка-якорь записана ДО касания диска.
        Err(_) => {
            // INSERT отбит (вероятно UNIQUE-дубль ключа) → ветвимся по replay_decision (по outcome).
            match ledger.replay_decision(&key).await {
                Ok(ReplayDecision::AlreadyDone(outcome)) => {
                    // Уже исполнено ранее — диск НЕ трогаем, отдаём прежний исход (идемпотентно).
                    return ApplyOutcome::AlreadyDone(outcome);
                }
                Ok(ReplayDecision::CrashedMidExecute(row)) => {
                    // Краш между write-before и finish: re-check on-disk hash vs строка. Дрейф ⇒ Failed
                    // (не клобберим); совпадение ⇒ complete-forward (доводим ту же запись).
                    if row.content_hash.as_deref() != on_disk_hash.as_deref() {
                        let _ = ledger
                            .finish(&key, STATE_FAILED, &drift_msg(&rel), None)
                            .await;
                        return ApplyOutcome::Failed(drift_msg(&rel));
                    }
                    false
                }
                Ok(ReplayDecision::Fresh) => {
                    // INSERT упал, но ключа нет — не UNIQUE-дубль, а реальный сбой записи ledger.
                    return ApplyOutcome::Failed(spec.ledger_write_fail.to_string());
                }
                Err(e) => return ApplyOutcome::Failed(format!("ledger replay: {e}")),
            }
        }
    };

    // ── РУБЕЖ 2 (вердикт existence) — ТОЛЬКО для Fresh (replay уже отбит). Заметка: create-над-сущ. /
    // edit-над-отсутств. ⇒ finish(Failed) (строка-якорь уже есть). Навык: вердикта нет (data-determined).
    // complete-forward (is_fresh=false) этот гейт ПРОПУСКАЕТ (create-файл уже наш → «довести», не «есть»).
    if is_fresh {
        if let Some(m) =
            (spec.existence_verdict)(action, is_create, current_content.is_some(), &rel)
        {
            let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
            return ApplyOutcome::Failed(m);
        }
    }

    // С этой точки строка-якорь в ledger существует. Любой ранний выход ОБЯЗАН finish(Failed).

    // ── РУБЕЖ 5: snapshot-before-act (manual=true → байпас 90с-троттла: снапшот на КАЖДУЮ правку, даже
    // rapid-fire). overwrite ⇒ фиксируем ПРЕД-правочное содержимое (обратимость); create ⇒ undo = Trash
    // (откат create = move_to_trash записанного файла). Снапшот не записался/ошибка ⇒ БЕЗ корректного undo
    // НЕ пишем файл (fail-closed): finish(Failed), диск не трогаем.
    let undo: UndoHandle = if is_create {
        UndoHandle::Trash {
            trash_rel: rel.clone(),
        }
    } else {
        let current = current_content.clone().unwrap_or_default();
        let snap = {
            let root = root.to_path_buf();
            let rel_s = rel.clone();
            tokio::task::spawn_blocking(move || {
                crate::vault::history::snapshot(&root, &rel_s, &current, true)?;
                // Новейший ts — отметка только что записанного снапшота.
                let snaps = crate::vault::history::list_snapshots(&root, &rel_s)?;
                Ok::<_, crate::vault::VaultError>(snaps.first().map(|s| s.ts))
            })
            .await
        };
        match snap {
            Ok(Ok(Some(ts))) => UndoHandle::Snapshot {
                rel: rel.clone(),
                // history ts — unix-МС (u64); UndoHandle::ts — i64. Сужение безопасно (< 2^63).
                ts: ts as i64,
            },
            Ok(Ok(None)) => {
                let m = (spec.snapshot_fail_msg)(&rel, SnapFail::NotWritten);
                let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
                return ApplyOutcome::Failed(m);
            }
            Ok(Err(e)) => {
                let m = (spec.snapshot_fail_msg)(&rel, SnapFail::Failed(&e));
                let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
                return ApplyOutcome::Failed(m);
            }
            Err(join_err) => {
                let m = (spec.snapshot_fail_msg)(&rel, SnapFail::Join(&join_err));
                let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
                return ApplyOutcome::Failed(m);
            }
        }
    };

    // ── Fix 2: RE-READ-BEFORE-WRITE drift-фенс (БЕЗУСЛОВНЫЙ, не зависит от classify_hash). РУБЕЖ 3
    // срабатывает ТОЛЬКО при classify_hash=Some; 3c-путь передаёт None → внешняя правка в окне read→write
    // молча затёрлась бы (стейл-снапшот + клоббер). Для OVERWRITE НЕПОСРЕДСТВЕННО перед atomic_write
    // перечитываем диск и сверяем с on_disk_hash (Рубеж 2); рассинхрон ⇒ Failed(drift), НЕ клобберим.
    let is_overwrite = !is_create && current_content.is_some();
    if is_overwrite && reread_drift_detected(&abs, on_disk_hash.as_deref()).await {
        let m = drift_msg(&rel);
        let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
        return ApplyOutcome::Failed(m);
    }

    // ── РУБЕЖ 6: WRITE (atomic_write по `abs` — НИКОГДА по root.join(rel)). Байты/ошибку даёт spec.
    let write_result: Result<(), String> =
        match (spec.build_write)(action, current_content.as_deref().unwrap_or("")) {
            Ok(bytes) => spawn_atomic_write(abs, bytes).await,
            Err(e) => Err(e),
        };

    // ── РУБЕЖ 7: ledger FINISH (поглощающий).
    match write_result {
        Ok(()) => {
            let summary = (spec.success_summary)(action, &rel, is_create);
            let _ = ledger
                .finish(&key, audit::STATE_EXECUTED, &summary, Some(undo.to_cols()))
                .await;
            ApplyOutcome::Executed { summary, undo }
        }
        Err(e) => {
            // Write упал ПОСЛЕ снапшота/ledger-строки: Failed без undo (atomic_write — всё-или-ничего).
            let _ = ledger.finish(&key, STATE_FAILED, &e, None).await;
            ApplyOutcome::Failed(e)
        }
    }
}

/// КАНОНИЗ-РУБЕЖ для записи ПО СУЩЕСТВУЮЩЕМУ файлу (overwrite). Единый источник конфайнмента, который
/// переиспользует и apply (overwrite-путь), и AGENT-4 undo (restore-снапшота — это ТОЖЕ запись в vault).
/// СИНХРОННЫЙ (вызывать из `spawn_blocking`). Порядок рубежей строгий:
///   1. `resolve_vault_path_for_write` — канонизирует РОДИТЕЛЯ и сверяет `starts_with(canon_root)`:
///      ловит симлинк-КАТАЛОГ наружу (классификатор лексически слеп к нему).
///   2. leaf-СИМЛИНК reject (`symlink_metadata` без следования): `vault/evil.md -> /outside/secret.md`
///      имеет родителя ВНУТРИ, но сам — симлинк наружу; без этой проверки запись/чтение утекли бы наружу.
///   3. ХАРДЛИНК reject (link-count > 1): хардлинк на внешний inode симлинком НЕ детектится; запись по
///      нему меняла бы внешний файл, чтение утекло бы внешним содержимым → `PathEscape`. Кросс-платформ
///      (AGENT-6): unix — `nlink>1` (`MetadataExt`); Windows — `nNumberOfLinks>1` через
///      `GetFileInformationByHandle` (std не даёт переносимого link-count). Это закрывает Windows-щель,
///      где раньше check был ТОЛЬКО под unix → пред-существующий хардлинк наружу не link-count-режектился
///      (запись не могла «убежать» благодаря rename-семантике atomic_write, но READ снапшота/диффа мог
///      затянуть внешнее содержимое в снапшот — info-leak). Теперь обе платформы режектят fail-closed.
///
/// Любой Err ⇒ caller НЕ пишет/НЕ читает по пути. Это keystone-рубеж: restore НИКОГДА не обходит его.
///
/// NB: применимо к ПЕРЕЗАПИСИ существующего leaf'а (overwrite/restore-снапшота). Для create (leaf'а ещё
/// нет) симлинк-leaf отсутствует, а симлинк-КОМПОНЕНТ родителя ловит [`reject_symlinked_components`] ДО
/// create_dir_all — поэтому create-путь в apply делает рубеж 1 отдельно (после возможного create_dir_all).
pub(in crate::actuator) fn confine_for_overwrite(
    canon_root: &Path,
    rel: &Path,
) -> Result<PathBuf, crate::vault::VaultError> {
    // Рубеж 1: канонизируем родителя + starts_with(canon_root) (симлинк-каталог наружу → PathEscape).
    let abs = crate::vault::resolve_vault_path_for_write(canon_root, rel)?;
    // Рубеж 2: leaf-симлинк (symlink_metadata НЕ следует). Нет файла → не симлинк → ок (но для restore
    // overwrite файл обычно есть). Симлинк наружу/внутрь — отвергаем fail-closed (легитимных нет).
    if let Ok(meta) = std::fs::symlink_metadata(&abs) {
        if meta.file_type().is_symlink() {
            return Err(crate::vault::VaultError::PathEscape);
        }
    }
    // Рубеж 3 (unix): хардлинк на внешний inode (nlink>1) — defense-in-depth против записи/info-leak.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(&abs) {
            if meta.is_file() && meta.nlink() > 1 {
                return Err(crate::vault::VaultError::PathEscape);
            }
        }
    }
    // Рубеж 3 (windows, AGENT-6): зеркало unix-проверки. std не даёт переносимого link-count, поэтому
    // читаем `nNumberOfLinks` через `GetFileInformationByHandle`. >1 ⇒ хардлинк на (возможно внешний)
    // inode ⇒ PathEscape (fail-closed) — закрывает Windows info-leak-щель (READ снапшота/диффа через
    // хардлинк наружу). Существующий-файл случай; отсутствие/ошибка открытия трактуем как «не хардлинк»
    // (нет файла → нечего режектить; реальную запись всё равно гейтят рубежи 1–2 + atomic_write rename).
    #[cfg(windows)]
    {
        if windows_link_count_gt_one(&abs) {
            return Err(crate::vault::VaultError::PathEscape);
        }
    }
    Ok(abs)
}

/// (windows) `nNumberOfLinks > 1` для существующего файла `abs` — хардлинк-детект (зеркало unix
/// `nlink>1`). Открывает хэндл с `FILE_FLAG_BACKUP_SEMANTICS` (работает и для путей-каталогов, не
/// следует за reparse без него — но leaf-симлинк уже отвергнут рубежом 2), читает
/// `BY_HANDLE_FILE_INFORMATION` и сверяет счётчик ссылок. Любая ошибка (файла нет / нет доступа) ⇒
/// `false` (не хардлинк / нечего режектить) — fail-safe-к-доступности, не к безопасности: запись всё
/// равно идёт через atomic_write (rename заменяет dir-entry, НЕ пишет сквозь линк), а READ под
/// снапшот защищён тем, что при хардлинке мы сюда вернём true ВЫШЕ и caller не прочитает.
#[cfg(windows)]
fn windows_link_count_gt_one(abs: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };

    // Путь → NUL-терминированный UTF-16 для широкого WinAPI.
    let mut wide: Vec<u16> = abs.as_os_str().encode_wide().collect();
    wide.push(0);

    // SAFETY: `wide` — валидный NUL-терминированный буфер на время вызова; остальные аргументы —
    // константы/нулевые указатели по контракту CreateFileW. Дескрипторы безопасности не используем
    // (NULL). Хэндл закрываем ниже на ВСЕХ путях.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0, // запрашиваем НОЛЬ прав доступа — достаточно для метаданных (link count), не открывает на чтение содержимого.
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING, // только существующий файл (как unix-проверка по metadata).
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return false; // файла нет / нет доступа → не хардлинк (нечего режектить).
    }

    // SAFETY: нулевая инициализация POD-структуры WinAPI допустима (она заполняется вызовом ниже).
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` валиден (проверен выше), `info` — валидный &mut на стеке.
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    // SAFETY: `handle` валиден; закрываем ровно один раз.
    unsafe {
        CloseHandle(handle);
    }
    // ok==0 → вызов не удался → не делаем вид, что знаем link-count → false (fail-safe к доступности).
    ok != 0 && info.nNumberOfLinks > 1
}

/// Fix 1 — симлинк-безопасность ПЕРЕД create_dir_all: проходит компоненты РОДИТЕЛЯ цели от `canon_root`
/// вниз и отвергает (`PathEscape`), если любой УЖЕ СУЩЕСТВУЮЩИЙ компонент — симлинк. Это закрывает дыру,
/// где `create_dir_all` следует по предсуществующему симлинку-каталогу наружу vault и создаёт пустые
/// каталоги ВНЕ vault до конфайнмент-проверки. `symlink_metadata` НЕ следует по симлинку (детектит сам
/// компонент). Несуществующие компоненты пропускаем (их создаст create_dir_all уже как свежие каталоги —
/// они не могут быть симлинками). Leaf (сам файл) НЕ проверяем — это забота родителя/resolve/рубежа 1.
fn reject_symlinked_components(
    canon_root: &Path,
    rel: &Path,
) -> Result<(), crate::vault::VaultError> {
    let parent = match rel.parent() {
        Some(p) => p,
        None => return Ok(()), // leaf в корне vault — нет промежуточных компонентов.
    };
    let mut cur = canon_root.to_path_buf();
    for comp in parent.components() {
        // Берём ТОЛЬКО Normal-компоненты: `..`/абсолют/префиксы классификатор уже отсёк (tools.rs),
        // а resolve_vault_path_for_write — бэкстоп; здесь идём строго вглубь от canon_root.
        if let std::path::Component::Normal(name) = comp {
            cur.push(name);
            // Только СУЩЕСТВУЮЩИЕ компоненты могут быть симлинком (симлинк — это запись на диске).
            if let Ok(meta) = std::fs::symlink_metadata(&cur) {
                if meta.file_type().is_symlink() {
                    return Err(crate::vault::VaultError::PathEscape);
                }
            }
        }
    }
    Ok(())
}

/// Полезная нагрузка действия для canonical_args/target_hash: `content` (тело) для create/edit,
/// `value` (значение ключа) для frontmatter. Единый источник, чтобы ключ был стабилен.
fn action_payload(action: &Action) -> Option<&str> {
    match &action.target {
        // SkillSave — content-несущая запись (тело SKILL.md), как create/edit: payload = content
        // (стабильный idempotency_key для skills-строки ledger'а; apply_skill_save переиспользует, SL-7c).
        ActionTarget::NoteCreate { .. }
        | ActionTarget::NoteEdit { .. }
        | ActionTarget::SkillSave { .. } => action.content.as_deref(),
        ActionTarget::Frontmatter { .. } => action.value.as_deref(),
        // exec не имеет content/value payload (и отсечён top-guard'ом до сюда) → None (безвредно).
        ActionTarget::ShellRun { .. }
        | ActionTarget::ProcessSpawn { .. }
        | ActionTarget::GitOp { .. } => None,
    }
}

/// Человеко-читаемое резюме успеха для tool-результата.
fn success_summary(action: &Action, rel: &str) -> String {
    match &action.target {
        ActionTarget::NoteCreate { .. } => format!("создана заметка {rel}"),
        ActionTarget::NoteEdit { .. } => format!("отредактирована заметка {rel}"),
        ActionTarget::Frontmatter { key, .. } => {
            format!("установлено свойство «{key}» в заметке {rel}")
        }
        // ПРОВАБЛИ-МЁРТВО: success_summary зовётся только на Executed-пути, exec туда не доходит (top-guard).
        ActionTarget::ShellRun { .. }
        | ActionTarget::ProcessSpawn { .. }
        | ActionTarget::GitOp { .. } => {
            unreachable!(
                "exec-таргет не доходит до success_summary (отсечён top-guard apply_action)"
            )
        }
        // SL-7: SkillSave идёт через apply_skill_save (своё резюме), apply_action его отсекает top-guard'ом.
        ActionTarget::SkillSave { .. } => {
            unreachable!("SkillSave не доходит до success_summary apply_action (top-guard 0-bis)")
        }
    }
}

/// Чтение файла в строку: `Some(текст)` если файл есть, `None` если нет (любая ошибка чтения —
/// трактуем как «нет/недоступен», fail-closed: дальше existence-проверка решит). Блокирующее IO в пуле.
async fn read_to_string_opt(abs: &Path) -> Option<String> {
    let abs = abs.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::read_to_string(&abs).ok())
        .await
        .ok()
        .flatten()
}

/// Fix 2 — ре-рид-перед-записью: ПЕРЕЧИТАТЬ on-disk файл и сравнить его хеш с `expected` (хешем
/// `current_content`, снятым в Рубеже 2). `true` ⇒ ДРЕЙФ (внешняя правка/удаление в окне read→write) —
/// caller отменяет запись (не клобберит чужое). БЕЗУСЛОВНЫЙ фенс: не зависит от classify_hash. Удалённый
/// файл → fresh=None → None != Some(expected) ⇒ дрейф (тоже отменяем — править нечего/гонка удаления).
async fn reread_drift_detected(abs: &Path, expected: Option<&str>) -> bool {
    let fresh = read_to_string_opt(abs).await;
    let fresh_hash = fresh
        .as_ref()
        .map(|c| crate::vault::content_hash(c.as_bytes()));
    fresh_hash.as_deref() != expected
}

/// Атомарная запись (tmp→fsync→rename) в blocking-пуле. Ошибку нормализуем в строку.
async fn spawn_atomic_write(abs: PathBuf, bytes: Vec<u8>) -> Result<(), String> {
    match tokio::task::spawn_blocking(move || crate::vault::atomic_write(&abs, &bytes)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("atomic_write: {e}")),
        Err(e) => Err(format!("atomic_write join: {e}")),
    }
}

#[cfg(test)]
mod tests;
