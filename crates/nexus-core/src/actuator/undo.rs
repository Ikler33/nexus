//! Движок ОБРАТИМОСТИ актуатора (AGENT-4, Фаза 1) — откат прогона по UndoHandle'ам, которые оставил 3c.
//!
//! [`undo_run`] идёт по применённым действиям прогона В ОБРАТНОМ ПОРЯДКЕ (newest-first) и восстанавливает
//! каждое через его [`UndoHandle`]:
//! - **Snapshot{rel, ts}** (откат edit/frontmatter): читаем пред-правочный снапшот и ПЕРЕЗАПИСЫВАЕМ заметку
//!   им — она возвращается к содержимому ДО правки;
//! - **Trash{trash_rel}** (откат create): переносим созданный файл в vault-корзину (`move_to_trash`) —
//!   «раздел» создания: файла больше нет в дереве.
//!
//! ## RESTORE — ЭТО ТОЖЕ ЗАПИСЬ В VAULT ⇒ КАНОНИЗ-РУБЕЖ ОБЯЗАТЕЛЕН
//! Snapshot-restore — `atomic_write` по пути заметки. Он ОБЯЗАН идти через [`apply::confine_for_overwrite`]
//! (тот же рубеж, что и 3c-apply на overwrite: `resolve_vault_path_for_write` канонизирует родителя →
//! leaf-симлинк reject → хардлинк reject). Если `target_rel` действия теперь резолвится СКВОЗЬ симлинк
//! наружу vault, restore ОТВЕРГАЕТСЯ ([`UndoStatus::PathEscape`]) и НИЧЕГО не пишется вне vault. Мы НИКОГДА
//! не пишем по `canon_root.join(rel)` напрямую — только по `abs`, который вернул рубеж. Trash-uncreate тоже
//! резолвит путь через тот же рубеж ПЕРЕД `move_to_trash` (rename вне vault недопустим).
//!
//! ## Идемпотентность отката
//! [`audit::actions_for_undo`] возвращает ТОЛЬКО строки в state `executed` (newest-first). После успешного
//! restore [`audit::mark_undone`] переводит строку `executed → undone` — на следующем `undo_run` её уже нет
//! в наборе (фильтр по state) → повтор = no-op. Mark идёт ПОСЛЕ успешной restore (порядок «эффект →
//! пометка»: краш между ними оставит строку executed → следующий undo повторит restore, что безопасно —
//! restore-снапшота и move_to_trash самоидемпотентны: повторная запись того же контента / уже-в-корзине).
//!
//! ## Толерантность к частичному провалу
//! Один сбойный откат (битый/отсутствующий снапшот, PathEscape) НЕ прерывает весь прогон: собираем
//! per-action исход в [`UndoOutcome`] и идём дальше. Caller (UI-1) покажет, что откатилось, а что нет.
//!
//! ## Граница AGENT-4 / UI-1 (НЕ здесь)
//! НЕТ tauri-команды, НЕТ UI-кнопки, НЕТ agentd-CLI входа — это UI-1. Движок дёргают тесты здесь; UI-1
//! навесит пользовательский триггер на ТЕ ЖЕ [`restore_snapshot`]/[`uncreate_via_trash`] (один путь
//! восстановления, много вызывающих). EventSink/стрим не трогаем.

use std::path::Path;

use super::apply::{confine_for_overwrite, AuditSink};
use super::audit;
use super::{UndoDomain, UndoHandle};

/// Статус отката ОДНОГО действия (per-action исход). Толерантно к частичному провалу: `undo_run` собирает
/// эти статусы и НЕ прерывается на первом сбое.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoStatus {
    /// Действие успешно откачено (снапшот восстановлен / created-файл в корзине) и помечено `undone`.
    Restored,
    /// Откатывать нечего — эффект уже снят (created-файл отсутствует / уже в корзине). Idempotent-success:
    /// строку всё равно помечаем `undone`. Отдельно от `Restored` ради наблюдаемости (для UI).
    AlreadyGone,
    /// Канониз-рубеж отверг путь (`target_rel` резолвится симлинком/хардлинком наружу vault) — НИ ОДНОГО
    /// write/rename вне vault. Строка НЕ помечается undone (откат не состоялся, fail-closed).
    PathEscape,
    /// Откат не удался по иной причине (снапшот отсутствует/битый, IO записи/переноса, ledger). Диск НЕ
    /// изменён этим действием; строка НЕ помечается undone. `reason` — пояснение.
    Failed(String),
    /// Откат РАСПОЗНАН, но ОТЛОЖЕН (SANDBOX-6c-2h): exec-GitOp несёт восстановимый pre-op git-ref, однако
    /// реальный `git reset --hard <ref>` — доп. in-container exec под host-апрувом (6c-3, Tier-2 live).
    /// Строка НЕ помечается undone (откат ещё не выполнен — 6c-3 завершит по тому же ref). `reason` несёт
    /// ref-подсказку для UI/оператора. Отличён от `Failed` (это НЕ ошибка, а honest «пока не реализовано»).
    Deferred(String),
}

impl UndoStatus {
    /// Засчитан ли откат состоявшимся (строку надо пометить `undone`). `Restored`/`AlreadyGone` — да;
    /// `PathEscape`/`Failed`/`Deferred` — нет (откат не выполнен/отложен, повтор/завершение позже допустимо).
    fn is_success(&self) -> bool {
        matches!(self, UndoStatus::Restored | UndoStatus::AlreadyGone)
    }
}

/// Исход отката одного действия в составе прогона: ключ строки + что это было + статус. Несёт ровно то,
/// что нужно UI-1 для пер-строчного отчёта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionUndo {
    /// idempotency_key строки ledger (для корреляции/повторного запроса).
    pub idempotency_key: String,
    /// Vault-rel путь цели (для показа пользователю).
    pub target_rel: Option<String>,
    /// Имя инструмента (note_create|note_edit|frontmatter) — что откатывали.
    pub tool_name: String,
    /// Per-action статус отката.
    pub status: UndoStatus,
    /// DRIFT-флаг (UI-1): ТЕКУЩЕЕ on-disk содержимое отличалось от того, к чему откат восстанавливает
    /// (т.е. вероятная правка ЧЕЛОВЕКА после прогона агента). НЕ блокирует откат — снапшот-перед делает
    /// перезапись не-деструктивной (перетёртое восстановимо из `.nexus/history`); это лишь сигнал для
    /// предупреждения пользователя. Для create-отката (uncreate) и не-успешных исходов — `false`.
    pub drifted: bool,
}

/// Агрегированный исход [`undo_run`]: per-action результаты (в ПОРЯДКЕ ОТКАТА — newest-first) + счётчики.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoOutcome {
    /// Прогон, который откатывали.
    pub run_id: i64,
    /// Per-action исходы в порядке отката (newest-first). Пусто ⇒ откатывать было нечего (no-op).
    pub actions: Vec<ActionUndo>,
}

impl UndoOutcome {
    /// Сколько действий реально откачено (Restored|AlreadyGone).
    pub fn restored(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| a.status.is_success())
            .count()
    }

    /// Сколько действий НЕ удалось откатить (PathEscape|Failed) — настоящие провалы. `Deferred` сюда НЕ
    /// входит (он не провал, а отложенный exec-GitOp откат — см. [`UndoOutcome::deferred`]). Считаем явно,
    /// а не вычитанием, чтобы deferred не маскировался под failed.
    pub fn failed(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| matches!(a.status, UndoStatus::PathEscape | UndoStatus::Failed(_)))
            .count()
    }

    /// Сколько действий ОТЛОЖЕНО (Deferred): exec-GitOp с зафиксированным pre-op ref, реальный reset —
    /// 6c-3. Не провал — honest «откат распознан, но ещё не выполнен».
    pub fn deferred(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| matches!(a.status, UndoStatus::Deferred(_)))
            .count()
    }

    /// Полностью ли откачен прогон: КАЖДОЕ действие успешно (ни провала, ни отложенного). Пустой набор
    /// (нечего откатывать) — тоже `true`. Эквивалентно `restored() == actions.len()`.
    pub fn fully_undone(&self) -> bool {
        self.failed() == 0 && self.deferred() == 0
    }
}

/// Шов исполнения exec-undo (SANDBOX-6c-3e): откат GitOp = САМ мутирующий GitOp, поэтому он RE-ENTER'ит
/// тот же host/exec путь (classify→decide→approve→mint-token→in-container execute→report), НЕ
/// привилегированный спец-путь. Для строки [`UndoHandle::ExecGitRef`] [`undo_run`] (с `driver` в [`UndoOpts`]) ре-валидирует
/// `reference` host-side ([`crate::sandbox::exec_host::is_git_sha`]) и зовёт [`UndoExecDriver::undo_gitref`].
/// Прод-impl (6c-3d `SandboxUndoExecDriver`) гоняет ОДИН полный цикл в `--network=none` контейнере под тем
/// же гейтом: `git reset --hard` классифицируется Confirm-НИКОГДА-Auto ⇒ под headless PolicyDefault
/// auto-DENY ⇒ откат остаётся `Deferred` честно; шелл/процесс НЕ имеют ExecGitRef-хэндла ⇒ драйвер для них
/// не зовётся (необратимы). `None` ⇒ поведение [`undo_run`] (Deferred surfacing — vault-only вызыватели).
#[async_trait::async_trait]
pub trait UndoExecDriver: Send + Sync {
    /// Откатить репозиторий к pre-op `reference` (валидный git-sha — ре-проверен вызывающим). Возвращает:
    /// [`UndoStatus::Restored`] (reset исполнен exit 0 ⇒ исходную строку пометят `undone`),
    /// [`UndoStatus::Deferred`] (апрув отклонён / undo-worktree не сконфигурирован ⇒ строка остаётся
    /// `executed`), [`UndoStatus::Failed`] (reset упал ⇒ строка остаётся, повтор допустим).
    async fn undo_gitref(&self, reference: &str) -> UndoStatus;
}

/// Опции отката прогона (R-12b): факультативные параметры [`undo_run`] сверх обязательных
/// `run_id`/`canon_root`/`ledger`. `Default` (== `UndoOpts::new()`) = ПРЕЖНЕЕ vault-only поведение
/// (skills_root=None, driver=None): exec-GitOp откат остаётся `Deferred`, строки навыков — Failed
/// (fail-closed). Билдер (`with_skills_root`/`with_driver`) добавляет ровно то, что нужно вызывателю —
/// вместо трёх врапперов с разными наборами None-хвостов (`undo_run`/`undo_run_full`/`undo_run_with_driver`).
#[derive(Default, Clone, Copy)]
pub struct UndoOpts<'a> {
    /// Корень навыков (SL-7c): строки навыков (`tool_name="skill_save"`) откатываются ПОД ним (НЕ vault
    /// `canon_root`). `None` ⇒ строка навыка не откатывается (Failed «skills_root не задан» — fail-closed:
    /// не угадываем корень). Прод-вызыватель (SL-7d/handler) подставляет канонизированный skills_root.
    pub skills_root: Option<&'a Path>,
    /// Драйвер exec-undo (SANDBOX-6c-3e): `Some` ⇒ exec-GitOp откат исполняется реально (синтезированный
    /// `git reset --hard <ref>` через песочницу под host-апрувом); `None` ⇒ `Deferred` surfacing (байт-в-байт
    /// как 6c-2h). Композиционный корень (agentd `--sandbox-undo`, 6c-3d) подставляет прод-драйвер.
    pub driver: Option<&'a dyn UndoExecDriver>,
}

impl<'a> UndoOpts<'a> {
    /// vault-only опции по умолчанию (без skills_root/driver) — прежнее поведение `undo_run`.
    pub fn new() -> Self {
        Self::default()
    }

    /// SL-7c: задать корень навыков для отката строк `skill_save` (иначе они fail-closed Failed).
    pub fn with_skills_root(mut self, skills_root: &'a Path) -> Self {
        self.skills_root = Some(skills_root);
        self
    }

    /// SANDBOX-6c-3e: задать драйвер реального exec-GitOp отката (иначе exec-GitOp остаётся Deferred).
    pub fn with_driver(mut self, driver: &'a dyn UndoExecDriver) -> Self {
        self.driver = Some(driver);
        self
    }
}

/// Откатить прогон `run_id`: пройти его применённые действия NEWEST-FIRST и восстановить каждое через его
/// [`UndoHandle`]. `canon_root` ОБЯЗАН быть уже канонизированным корнем vault (предусловие рубежа записи).
/// Факультативы (skills_root навыков, exec-undo драйвер) — через [`UndoOpts`]: `UndoOpts::new()` даёт
/// прежнее vault-only поведение (exec-GitOp откат остаётся `Deferred`, строки навыков — fail-closed Failed).
///
/// Reverse-order критичен: две правки одной заметки v0→v1→v2 откатываются v2 (снапшот=v1) ЗАТЕМ v1
/// (снапшот=v0) → итог v0. Откатив сначала старейшее, мы бы получили v1, а не v0.
///
/// Идемпотентен: повторный вызов видит пустой набор (откаченные строки в state `undone`, фильтр
/// [`audit::actions_for_undo`] их не вернёт) → no-op. Толерантен к частичному провалу: один сбойный откат
/// не прерывает остальные — собираем все исходы в [`UndoOutcome`]. `driver.reference` ре-валидируется
/// host-side ([`crate::sandbox::exec_host::is_git_sha`]) ПЕРЕД вызовом (инъекц-/мусор-ref ⇒ драйвер НЕ зовём).
pub async fn undo_run(
    run_id: i64,
    canon_root: &Path,
    ledger: &AuditSink,
    opts: UndoOpts<'_>,
) -> UndoOutcome {
    undo_run_inner(run_id, canon_root, opts.skills_root, ledger, opts.driver).await
}

/// Выбор корня отката строки по её ТИПИЗИРОВАННОМУ домену (R-12b): [`UndoDomain::Vault`] → vault
/// `canon_root`; [`UndoDomain::Skill`] → skills_root (если задан). SL-7c: навыки живут вне vault, их
/// Snapshot/Trash восстанавливаются под skills_root.
///
/// **Обратная совместимость.** Домен читается из ledger-поля `undo_domain`; для строк БЕЗ него (записаны
/// до R-12b) ИЛИ с битым значением — [`UndoDomain::from_tool_name`] (skill_save → Skill, иначе Vault),
/// ТОЧНОЕ зеркало прежней `tool_name`-эвристики → исторические ledger'ы читаются один-в-один.
fn undo_root_for<'a>(
    row: &audit::ActionRow,
    canon_root: &'a Path,
    skills_root: Option<&'a Path>,
) -> Option<&'a Path> {
    let domain = row
        .undo_domain
        .as_deref()
        .and_then(UndoDomain::parse)
        .unwrap_or_else(|| UndoDomain::from_tool_name(&row.tool_name));
    match domain {
        UndoDomain::Vault => Some(canon_root),
        UndoDomain::Skill => skills_root,
    }
}

async fn undo_run_inner(
    run_id: i64,
    canon_root: &Path,
    skills_root: Option<&Path>,
    ledger: &AuditSink,
    driver: Option<&dyn UndoExecDriver>,
) -> UndoOutcome {
    let reader = ledger.reader_handle();
    // Newest-first набор executed-строк с undo-хэндлом. Ошибка чтения ⇒ пустой исход (нечего откатывать
    // безопасно; fail-closed — не угадываем порядок без данных).
    let rows = match audit::actions_for_undo(&reader, run_id).await {
        Ok(rows) => rows,
        Err(_) => {
            return UndoOutcome {
                run_id,
                actions: Vec::new(),
            }
        }
    };

    let mut actions = Vec::with_capacity(rows.len());
    for row in rows {
        // Десериализуем хэндл из (undo_kind, undo_ref, target_rel). rel у Snapshot — из target_rel строки.
        let target_rel = row.target_rel.clone();
        let handle = match (row.undo_kind.as_deref(), row.undo_ref.as_deref()) {
            (Some(kind), Some(reference)) => {
                UndoHandle::from_cols(kind, reference, target_rel.as_deref().unwrap_or(""))
            }
            _ => None,
        };

        // (status, drifted): restore-снапшота сообщает дрейф (current != pre-edit ⇒ вероятная правка
        // человека); uncreate/битый хэндл дрейфа не несут (false).
        // R-12b: корень отката зависит от ТИПИЗИРОВАННОГО домена строки (undo_domain, tool_name-fallback
        // для старых): навык (Skill) → skills_root, заметка (Vault) → canon_root.
        let row_root = undo_root_for(&row, canon_root, skills_root);
        let (status, drifted) = match handle {
            Some(UndoHandle::Snapshot { rel, ts }) => match row_root {
                Some(root) => restore_snapshot(root, &rel, ts as u64).await,
                // Навык, но skills_root не задан (vault-only вызыватель) → откатить нечем (fail-closed).
                None => (
                    UndoStatus::Failed(format!(
                        "undo: навык {rel} — skills_root не задан, откат невозможен (fail-closed)"
                    )),
                    false,
                ),
            },
            Some(UndoHandle::Trash { trash_rel }) => {
                // Откат create: целевая заметка — это target_rel (где файл был создан). trash_rel в 3c
                // хранит ИМЕННО этот rel (намерение «перенести созданный файл в корзину»). Берём rel из
                // target_rel (источник истины пути), с fallback на trash_rel (они совпадают).
                let rel = target_rel.clone().unwrap_or(trash_rel);
                match row_root {
                    Some(root) => (uncreate_via_trash(root, &rel).await, false),
                    None => (
                        UndoStatus::Failed(format!(
                            "undo: навык {rel} — skills_root не задан, откат невозможен (fail-closed)"
                        )),
                        false,
                    ),
                }
            }
            // exec-GitOp откат (6c-2h/3e): pre-op ref из ledger. РЕ-ВАЛИДАЦИЯ host-side (defense-in-depth:
            // ledger мог быть повреждён/подменён — 6c-2h уже валидировал на записи, но не доверяем хранилищу
            // слепо): мусор-ref ⇒ Failed, драйвер НЕ зовём (никакого `git reset --hard <garbage>`). Валидный
            // ref: есть драйвер ⇒ реальный sandboxed-откат (Restored ⇒ строку пометят undone ниже); нет
            // драйвера ⇒ Deferred surfacing (vault-only вызыватели зовут undo_run без драйвера). НЕ vault-write.
            Some(UndoHandle::ExecGitRef { reference }) => {
                let status = if !crate::sandbox::exec_host::is_git_sha(&reference) {
                    UndoStatus::Failed(format!(
                        "exec-undo: невалидный git-ref в ledger ({reference:?}) — откат невозможен (fail-closed)"
                    ))
                } else if let Some(d) = driver {
                    d.undo_gitref(&reference).await
                } else {
                    UndoStatus::Deferred(format!(
                        "exec-GitOp откат отложен: восстановить можно `git reset --hard {reference}` в \
                         репозитории — авто-откат через песочницу под апрувом включается `--sandbox-undo` (6c-3d)"
                    ))
                };
                (status, false)
            }
            // Битый/неизвестный хэндл — откатить нечем (fail-closed). Не должно случаться (apply пишет
            // корректные), но не паникуем: помечаем Failed, идём дальше.
            None => (
                UndoStatus::Failed(format!(
                    "undo: не разобрать хэндл (kind={:?}, ref={:?})",
                    row.undo_kind, row.undo_ref
                )),
                false,
            ),
        };

        // Пометить строку undone ТОЛЬКО при состоявшемся откате (Restored|AlreadyGone). PathEscape/Failed
        // оставляют строку executed → повтор undo_run попробует снова (restore самоидемпотентен).
        if status.is_success() {
            // mark_undone сам идемпотентен (фенс executed→undone): гонка/повтор → no-op, не ошибка.
            let _ = audit::mark_undone(&ledger.writer_handle(), &row.idempotency_key).await;
        }

        actions.push(ActionUndo {
            idempotency_key: row.idempotency_key,
            target_rel,
            tool_name: row.tool_name,
            status,
            drifted,
        });
    }

    UndoOutcome { run_id, actions }
}

/// Восстановить заметку `rel` из её снапшота `ts` (откат edit/frontmatter). ПЕРЕИСПОЛЬЗУЕМ из UI-1.
///
/// RESTORE = ЗАПИСЬ В VAULT: путь резолвится через канониз-рубеж [`confine_for_overwrite`]
/// (resolve_vault_path_for_write → leaf-симлинк → хардлинк) — restore НИКОГДА его не обходит. Снимок
/// читается ИЗ `.nexus/history/<rel>/<ts>.md` (внутри vault, не пользовательский ввод пути). Затем
/// `atomic_write` пред-правочного содержимого по `abs` (НЕ по `canon_root.join(rel)`).
///
/// ## НЕ-ДЕСТРУКТИВНОСТЬ: снапшот ТЕКУЩЕГО содержимого ПЕРЕД перезаписью
/// Контракт undo — «вернуть к пред-правочному состоянию», но перезапись БЕЗУСЛОВНА: если ПОСЛЕ прогона
/// агента заметку правил ЧЕЛОВЕК, его правка была бы потеряна безвозвратно. Поэтому ДО `atomic_write`
/// пред-правочного содержимого мы СНАЧАЛА читаем ТЕКУЩЕЕ on-disk содержимое и снапшотим его через
/// `history::snapshot(.., manual=true)` (manual=true байпасит 90с-троттл — как apply на snapshot-before).
/// Тогда перезаписанное состояние ВСЕГДА восстановимо из `.nexus/history` (undo обратим, ничего не теряется;
/// без drift-fence: undo всё равно работает И ничего не теряется; без изменения схемы).
///
/// Порядок: confine → read current → snapshot(current, manual=true) → read_snapshot(pre-edit ts) →
/// atomic_write(pre-edit). Если текущего файла НЕТ (уже удалён) → снапшот пропускаем (сохранять нечего),
/// перезаписываем пред-правочным (восстановление). Если снапшот текущего НЕ удался (None/Err) → НЕ
/// перезаписываем без точки восстановления → [`UndoStatus::Failed`] (зеркалит apply «abort if snapshot
/// None/Err» — не теряем данные молча).
///
/// Исходы: снапшота нет/битый → [`UndoStatus::Failed`] (диск не тронут); путь резолвится наружу →
/// [`UndoStatus::PathEscape`] (ни одного write); снапшот текущего не удался → [`UndoStatus::Failed`] (диск
/// не тронут); успех → [`UndoStatus::Restored`]. `bool` в кортеже — DRIFT-флаг: текущее on-disk отличалось
/// от пред-правочного снапшота (вероятная правка человека), для предупреждения UI-1; на успех отката НЕ
/// влияет (снапшот-перед делает перезапись безопасной независимо).
pub(crate) async fn restore_snapshot(canon_root: &Path, rel: &str, ts: u64) -> (UndoStatus, bool) {
    let canon_root = canon_root.to_path_buf();
    let rel_s = rel.to_string();
    let res = tokio::task::spawn_blocking(move || {
        // 1) КАНОНИЗ-РУБЕЖ ДО ЛЮБОГО ЧТЕНИЯ/ЗАПИСИ: если target резолвится симлинком/хардлинком наружу —
        //    PathEscape, не читаем снапшот «в» внешний файл и не пишем наружу.
        let abs = match confine_for_overwrite(&canon_root, std::path::Path::new(&rel_s)) {
            Ok(abs) => abs,
            Err(_) => return (RestoreResult::PathEscape, false),
        };
        // 2) Прочитать пред-правочный снапшот (внутри vault). Нет/битый → Failed, диск не тронут.
        let content = match crate::vault::history::read_snapshot(&canon_root, &rel_s, ts) {
            Ok(c) => c,
            Err(e) => {
                return (
                    RestoreResult::Failed(format!(
                        "restore_snapshot {rel_s}@{ts}: снапшот недоступен ({e})"
                    )),
                    false,
                )
            }
        };
        // 3) НЕ-ДЕСТРУКТИВНОСТЬ: прочитать ТЕКУЩЕЕ on-disk содержимое и СНАПШОТНУТЬ его ДО перезаписи.
        //    Так перезаписанное (возможно — правка человека после прогона) всегда восстановимо из истории.
        //    Файл `abs` уже прошёл confine (Рубеж 1) — читаем по нему, не по canon_root.join(rel).
        match std::fs::read_to_string(&abs) {
            Ok(current) => {
                // Дрейф: текущее != пред-правочный снапшот ⇒ кто-то правил после прогона агента.
                let drifted = current != content;
                // snapshot(manual=true): байпас троттла (как apply на snapshot-before). Дедуп по контенту
                // внутри snapshot() безвреден — если current уже = последнему снапшоту, точка уже есть.
                // None ⇒ снапшот не записан И не было точки (нечего восстанавливать), Err ⇒ сбой истории:
                // в обоих случаях НЕ перезаписываем без точки восстановления (как apply abort if None/Err).
                match snapshot_current(&canon_root, &rel_s, &current) {
                    Ok(()) => {} // точка восстановления текущего содержимого зафиксирована
                    Err(m) => return (RestoreResult::Failed(m), drifted),
                }
                // 4) Атомарно ПЕРЕЗАПИСАТЬ заметку пред-правочным содержимым по `abs` (только по нему!).
                match crate::vault::atomic_write(&abs, content.as_bytes()) {
                    Ok(()) => (RestoreResult::Restored, drifted),
                    Err(e) => (
                        RestoreResult::Failed(format!("restore_snapshot {rel_s}: запись ({e})")),
                        drifted,
                    ),
                }
            }
            // Текущего файла нет (удалён вне vault / гонка): сохранять нечего → пропускаем снапшот-перед и
            // восстанавливаем пред-правочное содержимое. Дрейф = true (диск отличался от снапшота — пусто).
            Err(_) => match crate::vault::atomic_write(&abs, content.as_bytes()) {
                Ok(()) => (RestoreResult::Restored, true),
                Err(e) => (
                    RestoreResult::Failed(format!("restore_snapshot {rel_s}: запись ({e})")),
                    true,
                ),
            },
        }
    })
    .await;

    match res {
        Ok((RestoreResult::Restored, drift)) => (UndoStatus::Restored, drift),
        Ok((RestoreResult::PathEscape, drift)) => (UndoStatus::PathEscape, drift),
        Ok((RestoreResult::Failed(m), drift)) => (UndoStatus::Failed(m), drift),
        // restore-снапшота не порождает AlreadyGone (это исход uncreate); маппим консервативно.
        Ok((RestoreResult::AlreadyGone, drift)) => (UndoStatus::AlreadyGone, drift),
        Err(join) => (
            UndoStatus::Failed(format!("restore_snapshot join: {join}")),
            false,
        ),
    }
}

/// Снапшот ТЕКУЩЕГО on-disk содержимого `rel` ДО перезаписи отката (manual=true — байпас 90с-троттла,
/// как apply на snapshot-before). Возвращает `Err(msg)`, если точку восстановления СОЗДАТЬ НЕ УДАЛОСЬ
/// (сбой записи истории ИЛИ снапшот не появился) — тогда caller НЕ перезаписывает без recovery-точки
/// (зеркалит apply «abort if snapshot None/Err»). `Ok(())` — точка восстановления текущего содержимого
/// есть (записана сейчас ИЛИ уже существовала идентичной — дедуп snapshot()).
fn snapshot_current(canon_root: &Path, rel: &str, current: &str) -> Result<(), String> {
    // manual=true: фиксируем точку даже сразу после агентской правки (иначе 90с-троттл бы её пропустил).
    if let Err(e) = crate::vault::history::snapshot(canon_root, rel, current, true) {
        return Err(format!(
            "restore_snapshot {rel}: точка восстановления текущего содержимого не создана ({e}) — \
             перезапись отменена (обратимость не гарантирована)"
        ));
    }
    // Снапшот мог быть пропущен дедупом (current уже = последнему снапшоту) — это ОК, точка уже есть.
    // Но если истории нет ВООБЩЕ (снапшот не записался и точки нет) — recovery-точки нет → abort.
    match crate::vault::history::list_snapshots(canon_root, rel) {
        Ok(snaps) if !snaps.is_empty() => Ok(()),
        Ok(_) => Err(format!(
            "restore_snapshot {rel}: точка восстановления текущего содержимого отсутствует — \
             перезапись отменена (обратимость не гарантирована)"
        )),
        Err(e) => Err(format!(
            "restore_snapshot {rel}: проверка точки восстановления не удалась ({e}) — \
             перезапись отменена"
        )),
    }
}

/// Откатить create заметки `rel`: перенести созданный файл в vault-корзину (`move_to_trash`). ПЕРЕИСПОЛЬЗУЕМ
/// из UI-1.
///
/// Путь резолвится через тот же канониз-рубеж (rename наружу vault недопустим). Файла уже нет (повторный
/// undo / внешнее удаление) → [`UndoStatus::AlreadyGone`] (идемпотентность, не ошибка). Путь наружу →
/// [`UndoStatus::PathEscape`]. Успешный перенос → [`UndoStatus::Restored`].
pub(crate) async fn uncreate_via_trash(canon_root: &Path, rel: &str) -> UndoStatus {
    let canon_root = canon_root.to_path_buf();
    let rel_s = rel.to_string();
    let res = tokio::task::spawn_blocking(move || {
        // КАНОНИЗ-РУБЕЖ: resolve родителя + leaf-симлинк/хардлинк reject. Файла может уже не быть
        // (AlreadyGone) — резолв родителя всё равно отрабатывает (родитель существует), а leaf-проверка
        // (symlink_metadata) на отсутствующем файле = «не симлинк» → ок. Симлинк-каталог наружу → Err.
        let abs = match confine_for_overwrite(&canon_root, std::path::Path::new(&rel_s)) {
            Ok(abs) => abs,
            Err(_) => return RestoreResult::PathEscape,
        };
        // Файла нет → откатывать нечего (created-эффект уже снят) → AlreadyGone (идемпотентно).
        if !abs.exists() {
            return RestoreResult::AlreadyGone;
        }
        // Перенести созданный файл в корзину (`.nexus/.trash/…`) — rename в пределах vault, атомарен.
        match crate::vault::move_to_trash(&canon_root, &abs) {
            Ok(()) => RestoreResult::Restored,
            Err(e) => RestoreResult::Failed(format!("uncreate_via_trash {rel_s}: перенос ({e})")),
        }
    })
    .await;

    match res {
        Ok(RestoreResult::Restored) => UndoStatus::Restored,
        Ok(RestoreResult::AlreadyGone) => UndoStatus::AlreadyGone,
        Ok(RestoreResult::PathEscape) => UndoStatus::PathEscape,
        Ok(RestoreResult::Failed(m)) => UndoStatus::Failed(m),
        Err(join) => UndoStatus::Failed(format!("uncreate_via_trash join: {join}")),
    }
}

/// Внутренний исход restore-helper'а из blocking-пула (sync). Маппится в [`UndoStatus`] в async-обёртке.
enum RestoreResult {
    Restored,
    AlreadyGone,
    PathEscape,
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::action::Action;
    use crate::actuator::apply::{apply_action, apply_skill_save, ApplyOutcome};
    use crate::actuator::UNDO_EXEC_GITREF;
    use crate::db::Database;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Временный vault + БД; возвращает (dir, canon_root, AuditSink). canon_root КАНОНИЗИРОВАН
    /// (предусловие рубежа записи — на macOS /tmp → /private/tmp).
    async fn setup() -> (TempDir, PathBuf, AuditSink) {
        let dir = TempDir::new().unwrap();
        let canon_root = dir.path().canonicalize().unwrap();
        let db = Database::open(canon_root.join(".nexus/nexus.db"))
            .await
            .unwrap();
        let sink = AuditSink::new(db.writer().clone(), db.reader().clone());
        std::mem::forget(db); // актор жив пока жив клон writer/reader в sink (как в apply-тестах).
        (dir, canon_root, sink)
    }

    fn abs_of(root: &Path, rel: &str) -> PathBuf {
        root.join(rel)
    }

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(abs_of(root, rel)).unwrap()
    }

    fn write_existing(root: &Path, rel: &str, content: &str) {
        let abs = abs_of(root, rel);
        if let Some(p) = abs.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(abs, content).unwrap();
    }

    /// Применить действие через ТОТ ЖЕ прод-исполнитель `apply_action` (внутри crate::actuator он виден):
    /// он канонизирует путь, снапшотит ПРЕД-правочный контент (manual=true) и пишет executed-строку с
    /// корректным UndoHandle — ровно то состояние ledger/диска, которое откатывает undo_run. classify_hash
    /// = None (3c-путь), как у файловых инструментов.
    async fn apply_ok(action: &Action, run_id: i64, root: &Path, sink: &AuditSink) {
        let out = apply_action(action, run_id, root, sink, None).await;
        assert!(
            matches!(out, ApplyOutcome::Executed { .. }),
            "ожидалось Executed (иначе тест отката бессмыслен), получено {out:?}"
        );
    }

    /// EDIT → UNDO: применяем NoteEdit (снапшотит ПРЕД-edit контент) → undo_run → содержимое == ПРЕД-edit.
    #[tokio::test]
    async fn edit_then_undo_restores_pre_edit_content() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "Notes/E.md", "ORIGINAL");
        let action = Action::note_edit("Notes/E.md", "EDITED");
        apply_ok(&action, 1, &root, &sink).await;
        assert_eq!(read(&root, "Notes/E.md"), "EDITED", "правка применилась");

        let outcome = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(outcome.restored(), 1, "ровно одно действие откачено");
        assert!(outcome.fully_undone());
        assert_eq!(
            read(&root, "Notes/E.md"),
            "ORIGINAL",
            "снапшот восстановлен — заметка вернулась к ПРЕД-edit содержимому"
        );
    }

    /// НЕ-ДЕСТРУКТИВНОСТЬ (главный тест Fix 1): агент правит v0→v1, ЗАТЕМ ЧЕЛОВЕК правит v1→v2 НАПРЯМУЮ
    /// (мимо агента, прямой atomic_write — никакого ledger-действия). undo_run возвращает заметку к v0
    /// (пред-edit), НО правка человека v2 НЕ потеряна: она снапшотнута в `.nexus/history` ПЕРЕД перезаписью
    /// и восстановима (list_snapshots/read_snapshot содержат v2). Доказывает: undo обратим, ничего не теряется.
    #[tokio::test]
    async fn undo_is_nondestructive_snapshots_human_edit_before_overwrite() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "N.md", "v0");
        // Агент: v0 → v1 (снапшотит v0 как пред-edit точку отката).
        apply_ok(&Action::note_edit("N.md", "v1"), 1, &root, &sink).await;
        assert_eq!(read(&root, "N.md"), "v1", "агентская правка применилась");

        // ЧЕЛОВЕК правит заметку ПОСЛЕ прогона агента — НАПРЯМУЮ на диск, мимо актуатора/ledger.
        let abs = abs_of(&root, "N.md");
        crate::vault::atomic_write(&abs, b"v2-human").unwrap();
        assert_eq!(
            read(&root, "N.md"),
            "v2-human",
            "человек правит мимо агента"
        );

        // Откат прогона агента: контракт — вернуть к пред-edit (v0).
        let outcome = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(outcome.restored(), 1, "edit откачен");
        assert!(outcome.fully_undone());

        // (a) Пред-edit содержимое восстановлено.
        assert_eq!(read(&root, "N.md"), "v0", "undo вернул к пред-edit (v0)");

        // (b) ГЛАВНОЕ: правка человека v2 НЕ потеряна — снапшотнута ПЕРЕД перезаписью, восстановима.
        let snaps = crate::vault::history::list_snapshots(&root, "N.md").unwrap();
        let contents: Vec<String> = snaps
            .iter()
            .map(|s| crate::vault::history::read_snapshot(&root, "N.md", s.ts).unwrap())
            .collect();
        assert!(
            contents.iter().any(|c| c == "v2-human"),
            "перетёртая правка человека v2 восстановима из .nexus/history (undo НЕ-деструктивен): {contents:?}"
        );
        // Дрейф зафиксирован (current=v2 != pre-edit-снапшот=v0) — сигнал для UI-1.
        assert!(
            outcome.actions[0].drifted,
            "drift-флаг поднят: on-disk отличался от восстанавливаемого (правка человека)"
        );
    }

    /// CREATE → UNDO: применяем NoteCreate → undo_run → созданный файл ИСЧЕЗ из vault (перенесён в корзину).
    #[tokio::test]
    async fn create_then_undo_uncreates_file() {
        let (_d, root, sink) = setup().await;
        let action = Action::note_create("Notes/New.md", "fresh body");
        apply_ok(&action, 1, &root, &sink).await;
        assert!(abs_of(&root, "Notes/New.md").exists(), "файл создан");

        let outcome = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(outcome.restored(), 1);
        assert!(
            !abs_of(&root, "Notes/New.md").exists(),
            "create откачен — файла нет в дереве vault"
        );
        // Файл лежит в корзине (обратимо), а не уничтожен.
        let trash = root.join(".nexus/.trash");
        let in_trash = fs::read_dir(&trash)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with("New.md"));
        assert!(in_trash, "созданный файл перенесён в .nexus/.trash");
    }

    /// REVERSE ORDER: две правки одной заметки v0→v1→v2 (разные run? — нет, ОДИН прогон) → undo_run
    /// восстанавливает v0 (откат v2-снапшот=v1, затем v1-снапшот=v0 — newest-first). Итог == v0.
    #[tokio::test]
    async fn two_edits_undo_in_reverse_order_to_v0() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "R.md", "v0");
        // Обе правки в ОДНОМ прогоне (run_id=1) — undo_run откатывает весь прогон newest-first.
        apply_ok(&Action::note_edit("R.md", "v1"), 1, &root, &sink).await;
        apply_ok(&Action::note_edit("R.md", "v2"), 1, &root, &sink).await;
        assert_eq!(read(&root, "R.md"), "v2");

        let outcome = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(outcome.restored(), 2, "оба edit'а откачены");
        // Порядок отката — newest-first: первым в списке v2-правка (её снапшот = v1), затем v1 (снапшот=v0).
        assert_eq!(outcome.actions.len(), 2);
        assert!(outcome
            .actions
            .iter()
            .all(|a| a.status == UndoStatus::Restored));
        assert_eq!(
            read(&root, "R.md"),
            "v0",
            "reverse-order откат вернул заметку к ИСХОДНОМУ v0 (а не к v1)"
        );
    }

    /// IDEMPOTENT: undo_run дважды → второй no-op (набор пуст, откаченные строки в state=undone), контент
    /// стабилен, ошибок нет.
    #[tokio::test]
    async fn undo_run_twice_second_is_noop() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "I.md", "BEFORE");
        apply_ok(&Action::note_edit("I.md", "AFTER"), 1, &root, &sink).await;

        let first = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(first.restored(), 1);
        assert_eq!(read(&root, "I.md"), "BEFORE");

        let second = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert!(
            second.actions.is_empty(),
            "повторный undo_run — пустой набор (всё уже undone), no-op"
        );
        assert_eq!(
            read(&root, "I.md"),
            "BEFORE",
            "контент стабилен после повтора"
        );

        // Ledger: строка в state=undone (помечена), не воскрешена.
        let key = first.actions[0].idempotency_key.clone();
        let row = audit::lookup(&sink.reader_handle(), &key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.state, audit::STATE_UNDONE, "строка помечена undone");
    }

    /// PARTIAL FAILURE: у одного действия снапшот ОТСУТСТВУЕТ (битый ref) → ЭТО действие Failed, ДРУГОЕ
    /// (валидное) всё равно откачено. undo_run не прерывается на сбое.
    #[tokio::test]
    async fn partial_failure_one_fails_others_undone() {
        let (_d, root, sink) = setup().await;
        // Действие A: валидный edit (снапшот будет, откатится).
        write_existing(&root, "A.md", "A0");
        apply_ok(&Action::note_edit("A.md", "A1"), 1, &root, &sink).await;
        // Действие B: валидный edit (снапшот будет). Затем СПЕЦИАЛЬНО портим его undo_ref на
        // несуществующий ts → restore_snapshot не найдёт снапшот → Failed.
        write_existing(&root, "B.md", "B0");
        apply_ok(&Action::note_edit("B.md", "B1"), 1, &root, &sink).await;

        // Найдём B-строку и подменим её undo_ref на битый ts (снапшота с таким ts нет).
        let rows = audit::actions_for_undo(&sink.reader_handle(), 1)
            .await
            .unwrap();
        let b_key = rows
            .iter()
            .find(|r| r.target_rel.as_deref() == Some("B.md"))
            .unwrap()
            .idempotency_key
            .clone();
        sink.writer_handle()
            .transaction({
                let k = b_key.clone();
                move |tx| {
                    tx.execute(
                        "UPDATE agent_actions SET undo_ref='1' WHERE idempotency_key=?1",
                        rusqlite::params![k],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();

        let outcome = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(outcome.actions.len(), 2);
        assert_eq!(outcome.restored(), 1, "валидное A откачено");
        assert_eq!(outcome.failed(), 1, "битое B отчиталось провалом");
        // A восстановлено к A0; B осталось B1 (откат не состоялся).
        assert_eq!(
            read(&root, "A.md"),
            "A0",
            "валидное действие откачено несмотря на сбой соседа"
        );
        assert_eq!(
            read(&root, "B.md"),
            "B1",
            "сбойное действие НЕ изменило диск"
        );

        // B-строка осталась executed (НЕ помечена undone) → повтор undo попробует снова.
        let b_row = audit::lookup(&sink.reader_handle(), &b_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            b_row.state, "executed",
            "проваленный откат не помечает undone"
        );
        // B-исход — Failed.
        let b = outcome
            .actions
            .iter()
            .find(|a| a.target_rel.as_deref() == Some("B.md"))
            .unwrap();
        assert!(matches!(b.status, UndoStatus::Failed(_)), "B статус Failed");
    }

    /// RESTORE PATH-SAFETY (критический security-тест): снапшот существует под `evil.md`, НО на диске
    /// `evil.md` теперь — СИМЛИНК наружу vault. restore_snapshot ОБЯЗАН отвергнуть (PathEscape) и НЕ
    /// записать пред-правочное содержимое во ВНЕШНИЙ файл. Зеркалит 3c symlink-rampart на пути restore.
    #[cfg(unix)]
    #[tokio::test]
    async fn restore_rejects_symlink_out_path_escape() {
        use std::os::unix::fs::symlink;
        let (_d, root, _sink) = setup().await;
        // Внешний файл ВНЕ vault с известным содержимым.
        let outside_dir = TempDir::new().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap().join("secret.md");
        fs::write(&outside, "OUTSIDE-UNTOUCHED").unwrap();

        // Кладём «снапшот» для evil.md (как будто действие его сняло) с опасным содержимым.
        crate::vault::history::snapshot(&root, "evil.md", "PRE-EDIT-CONTENT", true).unwrap();
        let snaps = crate::vault::history::list_snapshots(&root, "evil.md").unwrap();
        let ts = snaps[0].ts;

        // Теперь evil.md на диске — симлинк наружу vault.
        symlink(&outside, abs_of(&root, "evil.md")).unwrap();

        // restore_snapshot ДОЛЖЕН отвергнуть путь (leaf-симлинк наружу) — ни одного write вне vault.
        let (status, _drift) = restore_snapshot(&root, "evil.md", ts).await;
        assert_eq!(
            status,
            UndoStatus::PathEscape,
            "restore по leaf-симлинку наружу → PathEscape, получено {status:?}"
        );
        // ГЛАВНЫЙ ИНВАРИАНТ: внешний файл НЕ перезаписан содержимым снапшота.
        assert_eq!(
            fs::read_to_string(&outside).unwrap(),
            "OUTSIDE-UNTOUCHED",
            "restore НЕ должен писать пред-правочное содержимое во внешний файл сквозь симлинк"
        );
        // Симлинк не заменён реальным файлом — мы вообще не писали.
        assert!(
            fs::symlink_metadata(abs_of(&root, "evil.md"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "симлинк не тронут (restore не выполнял write)"
        );
    }

    /// RESTORE PATH-SAFETY (симлинк-КАТАЛОГ): undo через target, чей родитель — симлинк наружу. Здесь
    /// побег ловит сам resolve_vault_path_for_write (канонизирует родителя наружу). uncreate-путь тоже
    /// обязан отвергнуть rename наружу vault.
    #[cfg(unix)]
    #[tokio::test]
    async fn uncreate_rejects_symlinked_parent_path_escape() {
        use std::os::unix::fs::symlink;
        let (_d, root, _sink) = setup().await;
        let outside_dir = TempDir::new().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap();
        // Подложим внешний файл, который НЕ должен уехать в корзину vault.
        fs::write(outside.join("x.md"), "OUTSIDE-FILE").unwrap();
        // Симлинк-каталог внутри vault → внешний каталог.
        symlink(&outside, abs_of(&root, "dirlink")).unwrap();

        // uncreate по dirlink/x.md: родитель резолвится наружу → PathEscape, move_to_trash не вызывается.
        let status = uncreate_via_trash(&root, "dirlink/x.md").await;
        assert_eq!(
            status,
            UndoStatus::PathEscape,
            "uncreate по симлинк-каталогу наружу → PathEscape, получено {status:?}"
        );
        assert!(
            outside.join("x.md").exists(),
            "внешний файл НЕ перенесён (rename наружу vault отвергнут)"
        );
    }

    /// IDEMPOTENT uncreate: повторный uncreate уже отсутствующего файла → AlreadyGone (не Failed).
    #[tokio::test]
    async fn uncreate_already_gone_is_success() {
        let (_d, root, _sink) = setup().await;
        // Файла нет вовсе.
        let status = uncreate_via_trash(&root, "Ghost.md").await;
        assert_eq!(
            status,
            UndoStatus::AlreadyGone,
            "uncreate отсутствующего файла идемпотентен (AlreadyGone)"
        );
    }

    /// 6c-2h: exec-GitOp откат в undo_run — Deferred (pre-op ref зафиксирован, реальный `git reset` — 6c-3).
    /// Строка НЕ помечается undone (6c-3 завершит); deferred()==1, НЕ провал, НЕ fully_undone; сообщение
    /// несёт ref-подсказку.
    #[tokio::test]
    async fn exec_gitref_undo_is_deferred() {
        let (_d, root, sink) = setup().await;
        // Засеять executed exec-GitOp строку с undo_kind=exec_gitref НАПРЯМУЮ (для exec нет vault-apply).
        let entry = audit::ActionEntry {
            run_id: 1,
            idempotency_key: "git-k".into(),
            tool_name: "git_op".into(),
            target_rel: None,
            risk_tier: "confirm".into(),
            state: audit::STATE_EXECUTING.into(),
            content_hash: None,
            diff_summary: None,
        };
        sink.record_before(entry).await.unwrap();
        sink.finish(
            "git-k",
            audit::STATE_EXECUTED,
            "exec exit=0",
            Some(audit::UndoCols {
                kind: UNDO_EXEC_GITREF.to_string(),
                reference: "cafebabe".into(),
                domain: None,
            }),
        )
        .await
        .unwrap();

        let outcome = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(outcome.actions.len(), 1);
        assert_eq!(outcome.deferred(), 1, "exec-GitOp откат отложен");
        assert_eq!(outcome.failed(), 0, "deferred — НЕ провал");
        assert_eq!(outcome.restored(), 0);
        assert!(
            !outcome.fully_undone(),
            "отложенный откат — не fully_undone"
        );
        match &outcome.actions[0].status {
            UndoStatus::Deferred(msg) => {
                assert!(msg.contains("cafebabe"), "ref-подсказка в сообщении: {msg}")
            }
            other => panic!("ожидался Deferred, получено {other:?}"),
        }
        // Строка осталась executed (НЕ undone) — 6c-3 завершит реальный reset по тому же ref.
        let row = audit::lookup(&sink.reader_handle(), "git-k")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "executed", "deferred НЕ помечает undone");
    }

    // ── 6c-3e: UndoExecDriver seam (реальный exec-GitOp откат через гейт) ─────────────────────────
    /// Засеять executed exec-GitOp строку (undo_kind=exec_gitref, ref=`reference`) НАПРЯМУЮ — для exec нет
    /// vault-apply. `reference` может быть невалидным (тест порчи/подмены ledger).
    async fn seed_exec_gitref(sink: &AuditSink, run_id: i64, key: &str, reference: &str) {
        let entry = audit::ActionEntry {
            run_id,
            idempotency_key: key.into(),
            tool_name: "git_op".into(),
            target_rel: None,
            risk_tier: "confirm".into(),
            state: audit::STATE_EXECUTING.into(),
            content_hash: None,
            diff_summary: None,
        };
        sink.record_before(entry).await.unwrap();
        sink.finish(
            key,
            audit::STATE_EXECUTED,
            "exec exit=0",
            Some(audit::UndoCols {
                kind: UNDO_EXEC_GITREF.to_string(),
                reference: reference.into(),
                domain: None,
            }),
        )
        .await
        .unwrap();
    }

    /// MockUndoExecDriver: скриптованный UndoStatus + флаг факта вызова (для «инъекц-ref → НЕ вызван»).
    /// Ассертит, что получает ВАЛИДНЫЙ ref (undo_run ре-валидирует ДО вызова — host-authority над ref).
    struct MockUndoExecDriver {
        status: UndoStatus,
        called: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait::async_trait]
    impl UndoExecDriver for MockUndoExecDriver {
        async fn undo_gitref(&self, reference: &str) -> UndoStatus {
            self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(
                crate::sandbox::exec_host::is_git_sha(reference),
                "драйвер получает ТОЛЬКО валидный ref (undo_run ре-валидирует): {reference:?}"
            );
            self.status.clone()
        }
    }
    fn mock_driver(
        status: UndoStatus,
    ) -> (
        MockUndoExecDriver,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        (
            MockUndoExecDriver {
                status,
                called: called.clone(),
            },
            called,
        )
    }

    async fn state_of(sink: &AuditSink, key: &str) -> String {
        audit::lookup(&sink.reader_handle(), key)
            .await
            .unwrap()
            .unwrap()
            .state
    }

    /// driver=None ⇒ exec-GitOp откат остаётся Deferred БАЙТ-в-байт (vault-only вызыватели, INV-DEFAULT-INERT).
    #[tokio::test]
    async fn exec_gitref_with_no_driver_still_deferred() {
        let (_d, root, sink) = setup().await;
        seed_exec_gitref(&sink, 1, "g", "a1b2c3d4").await;
        let outcome = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(outcome.deferred(), 1);
        assert_eq!(state_of(&sink, "g").await, "executed", "None ⇒ не undone");
    }

    /// Драйвер вернул Restored ⇒ исходную exec-GitOp строку помечают executed→undone; restored()==1.
    #[tokio::test]
    async fn exec_gitref_driver_restored_marks_undone() {
        let (_d, root, sink) = setup().await;
        seed_exec_gitref(&sink, 1, "g", "a1b2c3d4").await;
        let (driver, called) = mock_driver(UndoStatus::Restored);
        let outcome = undo_run(1, &root, &sink, UndoOpts::new().with_driver(&driver)).await;
        assert!(
            called.load(std::sync::atomic::Ordering::SeqCst),
            "драйвер вызван"
        );
        assert_eq!(outcome.restored(), 1, "exec-undo засчитан");
        assert!(outcome.fully_undone());
        assert_eq!(
            state_of(&sink, "g").await,
            audit::STATE_UNDONE,
            "исходная строка помечена undone ТОЛЬКО после Restored"
        );
    }

    /// Драйвер вернул Deferred (апрув отклонён под PolicyDefault) ⇒ строка остаётся executed; deferred()==1.
    #[tokio::test]
    async fn exec_gitref_driver_rejected_stays_deferred() {
        let (_d, root, sink) = setup().await;
        seed_exec_gitref(&sink, 1, "g", "a1b2c3d4").await;
        let (driver, _c) = mock_driver(UndoStatus::Deferred("апрув отклонён".into()));
        let outcome = undo_run(1, &root, &sink, UndoOpts::new().with_driver(&driver)).await;
        assert_eq!(outcome.deferred(), 1);
        assert_eq!(outcome.failed(), 0, "deferred — НЕ провал");
        assert_eq!(state_of(&sink, "g").await, "executed", "не undone");
    }

    /// Драйвер вернул Failed (reset упал) ⇒ строка остаётся executed; failed()==1 (повтор допустим).
    #[tokio::test]
    async fn exec_gitref_driver_failed_not_undone() {
        let (_d, root, sink) = setup().await;
        seed_exec_gitref(&sink, 1, "g", "a1b2c3d4").await;
        let (driver, _c) = mock_driver(UndoStatus::Failed("reset exit 1".into()));
        let outcome = undo_run(1, &root, &sink, UndoOpts::new().with_driver(&driver)).await;
        assert_eq!(outcome.failed(), 1);
        assert_eq!(state_of(&sink, "g").await, "executed", "не undone");
    }

    /// HOST-AUTHORITY над ref: ledger несёт ИНЪЕКЦ/мусор-ref (повреждён/подменён) ⇒ undo_run ре-валидирует
    /// is_git_sha → Failed, драйвер НЕ вызван (никакого `git reset --hard <garbage>`).
    #[tokio::test]
    async fn exec_gitref_invalid_ref_never_calls_driver() {
        let (_d, root, sink) = setup().await;
        seed_exec_gitref(&sink, 1, "g", "HEAD; rm -rf ~").await;
        let (driver, called) = mock_driver(UndoStatus::Restored);
        let outcome = undo_run(1, &root, &sink, UndoOpts::new().with_driver(&driver)).await;
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "инъекц-ref → драйвер НЕ вызван (fail-closed)"
        );
        assert_eq!(outcome.failed(), 1, "невалидный ref → Failed");
        assert_eq!(state_of(&sink, "g").await, "executed", "не undone");
    }

    /// shell.run (executed, undo_kind=None) НЕ в наборе отката ⇒ драйвер не зовётся (необратим структурно).
    #[tokio::test]
    async fn shell_exec_has_no_undo_handle() {
        let (_d, root, sink) = setup().await;
        let entry = audit::ActionEntry {
            run_id: 1,
            idempotency_key: "sh".into(),
            tool_name: "shell_run".into(),
            target_rel: None,
            risk_tier: "confirm".into(),
            state: audit::STATE_EXECUTING.into(),
            content_hash: None,
            diff_summary: None,
        };
        sink.record_before(entry).await.unwrap();
        sink.finish("sh", audit::STATE_EXECUTED, "exec exit=0", None)
            .await
            .unwrap();
        let (driver, called) = mock_driver(UndoStatus::Restored);
        let outcome = undo_run(1, &root, &sink, UndoOpts::new().with_driver(&driver)).await;
        assert!(
            outcome.actions.is_empty(),
            "shell без undo-хэндла — не в наборе отката"
        );
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "драйвер для shell не зовётся (необратим)"
        );
    }

    // ── SL-7c: откат навыков (skills_root-rooted Snapshot/Trash через undo_run_full) ─────────────
    const VALID_SKILL: &str = "---\nname: s\ndescription: d\n---\nBODY";

    /// Канонизированный отдельный skills_root внутри temp (НЕ vault canon_root).
    fn mk_skills_root(root: &Path) -> PathBuf {
        let p = root.join("skills_area");
        fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
    }

    async fn apply_skill_ok(action: &Action, run_id: i64, skills_root: &Path, sink: &AuditSink) {
        let never_paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let out = apply_skill_save(action, run_id, skills_root, sink, None, &never_paused).await;
        assert!(
            matches!(out, ApplyOutcome::Executed { .. }),
            "ожидалось Executed, получено {out:?}"
        );
    }

    /// CREATE навыка → undo_run_full (skills_root) уносит файл в корзину (откат create).
    #[tokio::test]
    async fn skill_create_then_undo_uncreates() {
        let (_d, root, sink) = setup().await;
        let skills_root = mk_skills_root(&root);
        apply_skill_ok(
            &Action::skill_save("s/SKILL.md", VALID_SKILL),
            1,
            &skills_root,
            &sink,
        )
        .await;
        assert!(skills_root.join("s/SKILL.md").exists(), "навык создан");

        let outcome = undo_run(
            1,
            &root,
            &sink,
            UndoOpts::new().with_skills_root(&skills_root),
        )
        .await;
        assert_eq!(outcome.restored(), 1, "create навыка откачен");
        assert!(
            !skills_root.join("s/SKILL.md").exists(),
            "созданный навык унесён из дерева (в корзину)"
        );
    }

    /// OVERWRITE навыка → undo_run_full восстанавливает ПРЕД-контент под skills_root.
    #[tokio::test]
    async fn skill_overwrite_then_undo_restores_prior() {
        let (_d, root, sink) = setup().await;
        let skills_root = mk_skills_root(&root);
        let old = "---\nname: s\ndescription: old\n---\nOLD";
        let abs = skills_root.join("s/SKILL.md");
        fs::create_dir_all(abs.parent().unwrap()).unwrap();
        fs::write(&abs, old).unwrap();

        apply_skill_ok(
            &Action::skill_save("s/SKILL.md", VALID_SKILL),
            1,
            &skills_root,
            &sink,
        )
        .await;
        assert_eq!(
            fs::read_to_string(&abs).unwrap(),
            VALID_SKILL,
            "перезаписан"
        );

        let outcome = undo_run(
            1,
            &root,
            &sink,
            UndoOpts::new().with_skills_root(&skills_root),
        )
        .await;
        assert_eq!(outcome.restored(), 1, "overwrite навыка откачен");
        assert_eq!(
            fs::read_to_string(&abs).unwrap(),
            old,
            "навык восстановлен к ПРЕД-контенту"
        );
    }

    /// FAIL-CLOSED: строка навыка, но undo_run (vault-only, без skills_root) → Failed, файл НЕ тронут
    /// (не угадываем корень). Восстановить можно только через undo_run_full со skills_root.
    #[tokio::test]
    async fn skill_undo_without_skills_root_fails_closed() {
        let (_d, root, sink) = setup().await;
        let skills_root = mk_skills_root(&root);
        apply_skill_ok(
            &Action::skill_save("s/SKILL.md", VALID_SKILL),
            1,
            &skills_root,
            &sink,
        )
        .await;

        // undo_run НЕ знает skills_root → навык откатить нечем.
        let outcome = undo_run(1, &root, &sink, UndoOpts::new()).await;
        assert_eq!(outcome.failed(), 1, "без skills_root навык → Failed");
        assert_eq!(outcome.restored(), 0);
        assert!(
            skills_root.join("s/SKILL.md").exists(),
            "файл навыка не тронут (fail-closed)"
        );
    }

    /// EMPTY RUN: прогон без откатываемых действий → undo_run no-op (пустой исход, fully_undone).
    #[tokio::test]
    async fn undo_run_empty_for_run_without_actions() {
        let (_d, root, sink) = setup().await;
        let outcome = undo_run(999, &root, &sink, UndoOpts::new()).await;
        assert!(outcome.actions.is_empty());
        assert!(
            outcome.fully_undone(),
            "пустой откат — полностью откачен (нечего откатывать)"
        );
        assert_eq!(outcome.restored(), 0);
    }

    // ── R-12b: типизированный домен корня (undo_domain) + обратная совместимость ────────────────────

    /// R-12b: СВЕЖИЕ строки несут ТИПИЗИРОВАННЫЙ домен в ledger — навык → `"skill"`, заметка → `"vault"`.
    /// Доказывает, что apply проставляет `undo_domain` по своей spec (SKILL_SPEC=Skill, NOTE_SPEC=Vault).
    #[tokio::test]
    async fn fresh_rows_persist_typed_domain() {
        let (_d, root, sink) = setup().await;
        let skills_root = mk_skills_root(&root);
        // Навык → домен "skill".
        apply_skill_ok(
            &Action::skill_save("s/S.md", VALID_SKILL),
            1,
            &skills_root,
            &sink,
        )
        .await;
        let srows = audit::actions_for_undo(&sink.reader_handle(), 1)
            .await
            .unwrap();
        assert_eq!(
            srows[0].undo_domain.as_deref(),
            Some("skill"),
            "свежий навык → undo_domain=skill"
        );
        // Заметка → домен "vault".
        write_existing(&root, "N.md", "v0");
        apply_ok(&Action::note_edit("N.md", "v1"), 2, &root, &sink).await;
        let nrows = audit::actions_for_undo(&sink.reader_handle(), 2)
            .await
            .unwrap();
        assert_eq!(
            nrows[0].undo_domain.as_deref(),
            Some("vault"),
            "свежая заметка → undo_domain=vault"
        );
    }

    /// R-12b ОБРАТНАЯ СОВМЕСТИМОСТЬ (обязательный тест на ИСТОРИЧЕСКОМ ФИКСТУРЕ): строка навыка,
    /// записанная ДО R-12b (поля `undo_domain` не было → NULL), откатывается ПРАВИЛЬНО через
    /// `tool_name`-fallback (`skill_save` → Skill → skills_root). Симулируем старую строку, ЗАНУЛИВ
    /// колонку у реально применённого навыка (её on-disk состояние совпадает со старым форматом).
    #[tokio::test]
    async fn old_row_without_undo_domain_falls_back_to_tool_name() {
        let (_d, root, sink) = setup().await;
        let skills_root = mk_skills_root(&root);
        apply_skill_ok(
            &Action::skill_save("s/SKILL.md", VALID_SKILL),
            1,
            &skills_root,
            &sink,
        )
        .await;
        assert!(skills_root.join("s/SKILL.md").exists(), "навык создан");

        // ИСТОРИЧЕСКИЙ ФИКСТУР: зануляем undo_domain — как будто строку записал бинарь ДО миграции 028
        // (колонки не было → NULL). Прогон helper'а below падает при добавлении новых столбцов — точечно.
        sink.writer_handle()
            .transaction(|tx| {
                tx.execute(
                    "UPDATE agent_actions SET undo_domain=NULL WHERE run_id=1",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let rows = audit::actions_for_undo(&sink.reader_handle(), 1)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].undo_domain.is_none(),
            "фикстур: undo_domain NULL (старый формат до R-12b)"
        );
        assert_eq!(rows[0].tool_name, "skill_save", "строка навыка");

        // Откат: несмотря на NULL-домен, fallback по tool_name даёт Skill → skills_root → файл унесён.
        let outcome = undo_run(
            1,
            &root,
            &sink,
            UndoOpts::new().with_skills_root(&skills_root),
        )
        .await;
        assert_eq!(
            outcome.restored(),
            1,
            "старый навык откачен через tool_name-fallback (undo_domain NULL)"
        );
        assert!(
            !skills_root.join("s/SKILL.md").exists(),
            "файл унесён из skills_root — fallback выбрал ПРАВИЛЬНЫЙ корень для исторической строки"
        );
    }

    /// R-12b: ЗАЩИТА fallback'а — старая строка ЗАМЕТКИ (undo_domain NULL, tool_name=note_edit) идёт под
    /// canon_root (Vault), НЕ под skills_root. Доказывает, что fallback не «всё в skills».
    #[tokio::test]
    async fn old_note_row_without_domain_restores_under_canon_root() {
        let (_d, root, sink) = setup().await;
        let skills_root = mk_skills_root(&root);
        write_existing(&root, "N.md", "ORIGINAL");
        apply_ok(&Action::note_edit("N.md", "EDITED"), 1, &root, &sink).await;
        // Зануляем домен (историческая строка).
        sink.writer_handle()
            .transaction(|tx| {
                tx.execute(
                    "UPDATE agent_actions SET undo_domain=NULL WHERE run_id=1",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        // Даже со skills_root в опциях заметка откатывается под canon_root (Vault-fallback).
        let outcome = undo_run(
            1,
            &root,
            &sink,
            UndoOpts::new().with_skills_root(&skills_root),
        )
        .await;
        assert_eq!(outcome.restored(), 1, "старая заметка откачена");
        assert_eq!(
            read(&root, "N.md"),
            "ORIGINAL",
            "восстановлена под canon_root (Vault-fallback), не под skills_root"
        );
    }
}
