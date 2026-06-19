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
pub async fn apply_action(
    action: &Action,
    run_id: i64,
    canon_root: &Path,
    ledger: &AuditSink,
    classify_hash: Option<&str>,
) -> ApplyOutcome {
    let rel = action.target.rel().to_string();

    // ── РУБЕЖ 1: canonicalize/symlink RAMPART (2-й рубеж к лексическому classify) ──────────────
    // resolve_vault_path_for_write канонизирует РОДИТЕЛЯ и проверяет starts_with(canon_root): следует
    // по симлинку-КАТАЛОГУ и ловит побег НАРУЖУ (`vault/dirlink/x` → dirlink канонизируется наружу →
    // PathEscape), который classify (лексический) не видит. Пишем ТОЛЬКО по `abs`.
    //
    // ВАЖНО (закрытие реального остаточного отверстия): resolve канонизирует РОДИТЕЛЯ, но НЕ сам leaf.
    // Симлинк-ЛИСТ `vault/evil.md -> /outside/secret.md` имеет родителя = vault-корень (ВНУТРИ), поэтому
    // resolve вернул бы путь leaf-симлинка как «in-vault». Чтение по нему утекло бы внешним содержимым в
    // снапшот, а на не-rename писателе запись прошла бы СКВОЗЬ симлинк наружу. Поэтому ДОПОЛНИТЕЛЬНО:
    // если leaf — симлинк, это PathEscape (у vault-заметок нет легитимных симлинков-листов; fail-closed).
    let is_create = matches!(action.target, ActionTarget::NoteCreate { .. });
    let abs = {
        let canon_root = canon_root.to_path_buf();
        let rel_path = PathBuf::from(&rel);
        let resolved = tokio::task::spawn_blocking(move || {
            // resolve_vault_path_for_write канонизирует НЕПОСРЕДСТВЕННОГО родителя цели — он ОБЯЗАН
            // существовать. Для create в НОВУЮ подпапку (`Notes/N.md`, `Notes/` ещё нет) родитель надо
            // создать ДО резолва. Создаём ТОЛЬКО для create и ТОЛЬКО лексический parent под canon_root;
            // классификатор уже отсёк `..`/абсолют/reserved (tools.rs), а резолв НИЖЕ всё равно отвергнет
            // побег через предсуществующий симлинк-каталог (канонизирует созданный parent и сверит с root).
            if is_create {
                if let Some(parent) = canon_root.join(&rel_path).parent() {
                    if !parent.exists() {
                        // СИМЛИНК-БЕЗОПАСНЫЙ create_dir_all (Fix 1): обычный create_dir_all СЛЕДУЕТ по
                        // симлинку в ПРЕДСУЩЕСТВУЮЩЕМ компоненте. Для нового глубокого пути `sub/a/b/n.md`,
                        // где `sub` — симлинк НАРУЖУ vault, create_dir_all создал бы `/outside/a/b` (пустые
                        // каталоги ВНЕ vault) ДО того, как resolve_vault_path_for_write отвергнет запись.
                        // Поэтому СНАЧАЛА проходим rel-компоненты родителя от canon_root вниз: любой уже
                        // СУЩЕСТВУЮЩИЙ компонент, который — симлинк (symlink_metadata без следования), ⇒
                        // PathEscape (у vault-заметок нет легитимных симлинков-каталогов; fail-closed).
                        // Только после этого create_dir_all создаёт СВЕЖИЕ каталоги (они не могут быть
                        // симлинками). ЕДИНСТВЕННЫЙ до-ledger дисковый эффект — пустые каталоги ВНУТРИ vault
                        // (review-принятый nit): симлинк-побег исключён этой проверкой ещё до записи.
                        reject_symlinked_components(&canon_root, &rel_path)?;
                        std::fs::create_dir_all(parent)?;
                    }
                }
            }
            let abs = crate::vault::resolve_vault_path_for_write(&canon_root, &rel_path)?;
            // symlink_metadata НЕ следует по симлинку — детектит САМ leaf-симлинк (в т.ч. на
            // несуществующую/внешнюю цель). Нет файла (create) → Err NotFound → не симлинк → ок.
            if let Ok(meta) = std::fs::symlink_metadata(&abs) {
                if meta.file_type().is_symlink() {
                    return Err(crate::vault::VaultError::PathEscape);
                }
            }
            // Fix 3 (defense-in-depth, OVERWRITE): hardlink-reject. symlink_metadata().is_symlink()
            // ЛОЖЕН для ХАРДЛИНКА на внешний inode (хардлинк — не симлинк). Чтение current_content по
            // такому листу утекло бы внешним содержимым в снапшот (info-leak), и хотя rename atomic_write
            // НЕ пишет СКВОЗЬ линк, fail-closed корректнее. Для существующего файла (overwrite) сверяем
            // число жёстких ссылок: nlink>1 ⇒ файл разделяет inode ещё с кем-то (вне vault) ⇒ PathEscape.
            // metadata (СЛЕДУЕТ по симлинку) тут безопасна — leaf-симлинк уже отвергнут выше.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if let Ok(meta) = std::fs::metadata(&abs) {
                    if meta.is_file() && meta.nlink() > 1 {
                        return Err(crate::vault::VaultError::PathEscape);
                    }
                }
            }
            Ok::<_, crate::vault::VaultError>(abs)
        })
        .await;
        match resolved {
            Ok(Ok(abs)) => abs,
            // Err от резолва (PathEscape: каталог-симлинк наружу ИЛИ leaf-симлинк / IO канонизации
            // родителя) ⇒ НИ ОДНОГО write, ни одного чтения по симлинку.
            Ok(Err(_)) => return ApplyOutcome::PathEscape,
            Err(join_err) => return ApplyOutcome::Failed(format!("apply join: {join_err}")),
        }
    };

    // ── РУБЕЖ 2: read-current (для hash/снапшота). Existence-ВЕРДИКТ выносим ПОСЛЕ ledger-фенса:
    // replay того же действия (create, файл уже создан 1-м прогоном) должен дать AlreadyDone, а не
    // «уже существует» — идемпотентность сильнее существования. Поэтому existence-gate (для Fresh) —
    // ниже, после record_before. read — это просто чтение (не вердикт), его делаем сейчас.
    let current_content: Option<String> = read_to_string_opt(&abs).await;
    let on_disk_hash: Option<String> = current_content
        .as_ref()
        .map(|c| crate::vault::content_hash(c.as_bytes()));

    // Идемпотентность-ключ: target_hash — отпечаток цели НА МОМЕНТ classify (часть тождества действия,
    // СТАБИЛЕН в окне краш→retry — иначе replay не сматчился бы). Если `classify_hash` задан (3d-путь
    // через changeset) — берём ЕГО (at-classify view). Если None (3c-инструмент без changeset) —
    // fallback: для create отпечаток ПЛАНИРУЕМОГО содержимого (стабилен между прогонами даже после того,
    // как 1-й прогон создал файл), для edit/frontmatter — on-disk hash сейчас. `content_hash` (колонка
    // ledger) — ОТДЕЛЬНЫЙ токен оптимистичной конкуренции (on-disk-at-attempt) для CrashedMidExecute
    // re-check; он НЕ часть ключа, поэтому дрейф диска не «уводит» ключ — даёт CrashedMidExecute.
    let payload = action_payload(action);
    let target_hash = match classify_hash {
        Some(h) => h.to_string(),
        None => match &action.target {
            ActionTarget::NoteCreate { .. } => {
                crate::vault::content_hash(payload.unwrap_or("").as_bytes())
            }
            _ => on_disk_hash
                .clone()
                .unwrap_or_else(|| crate::vault::content_hash(payload.unwrap_or("").as_bytes())),
        },
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

    // ── РУБЕЖ 4: ledger WRITE-BEFORE-ACT (ДО любого write) ────────────────────────────────────
    // INSERT строки-якоря (outcome=NULL) — это ФЕНС. Успех ⇒ Fresh (мы первые). Дубль (UNIQUE) ⇒ это
    // replay того же действия в прогоне → ветвимся по присутствию outcome.
    let entry = ActionEntry {
        run_id,
        idempotency_key: key.clone(),
        tool_name: action.target.tool_name().to_string(),
        target_rel: Some(rel.clone()),
        risk_tier: audit::TIER_AUTO.to_string(),
        state: STATE_EXECUTING.to_string(),
        content_hash: on_disk_hash.clone(),
        diff_summary: None,
    };
    let is_fresh = match ledger.record_before(entry).await {
        Ok(_) => true, // Fresh: строка-якорь записана ДО касания диска.
        Err(_) => {
            // INSERT отбит (вероятно UNIQUE-дубль ключа) → ветвимся по replay_decision (по outcome).
            match ledger.replay_decision(&key).await {
                Ok(ReplayDecision::AlreadyDone(outcome)) => {
                    // Уже исполнено ранее — НЕ переписываем диск, отдаём прежний исход (идемпотентно).
                    return ApplyOutcome::AlreadyDone(outcome);
                }
                Ok(ReplayDecision::CrashedMidExecute(row)) => {
                    // Краш между write-before и finish: re-check on-disk hash vs строка.
                    // Дрейф (диск изменился) ⇒ Failed (не клобберим); совпадение ⇒ complete-forward.
                    if row.content_hash.as_deref() != on_disk_hash.as_deref() {
                        let _ = ledger
                            .finish(&key, STATE_FAILED, &drift_msg(&rel), None)
                            .await;
                        return ApplyOutcome::Failed(drift_msg(&rel));
                    }
                    // hash совпал — состояние то же, что при старте крашнутой попытки → доводим write.
                    false
                }
                Ok(ReplayDecision::Fresh) => {
                    // INSERT упал, но ключа нет — не UNIQUE-дубль, а реальный сбой записи ledger.
                    return ApplyOutcome::Failed(
                        "ledger record_before: запись строки действия не удалась".to_string(),
                    );
                }
                Err(e) => return ApplyOutcome::Failed(format!("ledger replay: {e}")),
            }
        }
    };

    // ── РУБЕЖ 2 (вердикт existence) — ТОЛЬКО для Fresh (replay уже отбит выше). После фенса: если
    // create поверх существующего / edit по несуществующему — fail-closed + finish(Failed) (строка-якорь
    // уже есть). complete-forward (is_fresh=false) этот гейт ПРОПУСКАЕТ: для create файл уже создан
    // нами в крашнутой попытке — это и есть «довести», а не «уже существует».
    if is_fresh {
        if is_create && current_content.is_some() {
            let m = format!(
                "note.create: цель {rel} уже существует — создание отменено (используйте edit для правки)"
            );
            let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
            return ApplyOutcome::Failed(m);
        }
        if !is_create && current_content.is_none() {
            let m = format!(
                "{}: цель {rel} не существует — править нечего",
                action.target.tool_name()
            );
            let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
            return ApplyOutcome::Failed(m);
        }
    }

    // С этой точки строка-якорь в ledger существует. Любой ранний выход ОБЯЗАН finish(Failed) — иначе
    // строка останется CrashedMidExecute (корректно для replay, но для синхронного пути мы фиксируем).

    // ── РУБЕЖ 5: snapshot-before-act (manual=true → байпас 90с-троттла) ────────────────────────
    // Для overwrite (edit/frontmatter) фиксируем ПРЕД-правочное содержимое снапшотом ДО записи —
    // обратимость. manual=true критичен: иначе быстрые правки подряд молча схлопнулись бы троттлом и
    // часть undo-точек пропала бы. Для create undo = Trash (откат = move_to_trash записанного файла).
    let undo: UndoHandle = if is_create {
        // trash_rel — путь в vault-корзине. Реальный trash возникнет при undo (move_to_trash); здесь
        // фиксируем НАМЕРЕНИЕ: откат create = перенос созданного файла `rel` в корзину.
        UndoHandle::Trash {
            trash_rel: rel.clone(),
        }
    } else {
        let current = current_content.clone().unwrap_or_default();
        let snap = {
            let canon_root = canon_root.to_path_buf();
            let rel_s = rel.clone();
            tokio::task::spawn_blocking(move || {
                // manual=true — байпас троттла: снапшот на КАЖДУЮ правку, даже rapid-fire.
                crate::vault::history::snapshot(&canon_root, &rel_s, &current, true)?;
                // Новейший ts — отметка только что записанного снапшота.
                let snaps = crate::vault::history::list_snapshots(&canon_root, &rel_s)?;
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
            // Снапшот не записался (дедуп пустой правки/ошибка) → НЕ отдаём undo, который не восстановит.
            // Fail-closed: без корректного undo не пишем сам файл — finish(Failed), диск не трогаем.
            Ok(Ok(None)) => {
                let m = format!(
                    "snapshot-before {rel}: точка восстановления не создана — запись отменена \
                     (обратимость не гарантирована)"
                );
                let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
                return ApplyOutcome::Failed(m);
            }
            Ok(Err(e)) => {
                let m = format!("snapshot-before {rel}: {e}");
                let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
                return ApplyOutcome::Failed(m);
            }
            Err(join_err) => {
                let m = format!("snapshot join: {join_err}");
                let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
                return ApplyOutcome::Failed(m);
            }
        }
    };

    // ── Fix 2: RE-READ-BEFORE-WRITE drift-фенс (БЕЗУСЛОВНЫЙ, не зависит от classify_hash) ──────
    // `current_content` прочитан ОДИН раз в Рубеже 2 и переиспользован под снапшот + базу frontmatter +
    // edit. Drift-проверка Рубежа 3 срабатывает ТОЛЬКО при classify_hash=Some, а 3c-инструмент передаёт
    // None → конкурентная внешняя правка в окне read→write молча затёрлась бы (стейл-снапшот + клоббер).
    // Поэтому для OVERWRITE (edit/frontmatter, файл существовал) НЕПОСРЕДСТВЕННО перед atomic_write
    // ПЕРЕЧИТЫВАЕМ диск и сверяем хеш с хешем `current_content`, снятым ранее; рассинхрон ⇒ Failed(drift),
    // НЕ клобберим. Это зеркалит прод-гарду команды set_frontmatter_field (vault.rs). Снапшот Рубежа 5 уже
    // зафиксировал пред-правочное содержимое; этот ре-рид — именно фенс «прямо перед записью».
    let is_overwrite = !is_create && current_content.is_some();
    if is_overwrite && reread_drift_detected(&abs, on_disk_hash.as_deref()).await {
        // on_disk_hash = хеш content, снятого в Рубеже 2 (база этой правки). Любой рассинхрон (внешняя
        // правка ИЛИ внешнее удаление файла → fresh=None) ⇒ дрейф: отменяем, не затирая чужие правки.
        let m = drift_msg(&rel);
        let _ = ledger.finish(&key, STATE_FAILED, &m, None).await;
        return ApplyOutcome::Failed(m);
    }

    // ── РУБЕЖ 6: WRITE (atomic_write по `abs` — НИКОГДА по canon_root.join(rel)) ────────────────
    let write_result: Result<(), String> = match &action.target {
        ActionTarget::NoteCreate { .. } | ActionTarget::NoteEdit { .. } => {
            let bytes = action.content.clone().unwrap_or_default().into_bytes();
            let abs_w = abs.clone();
            spawn_atomic_write(abs_w, bytes).await
        }
        ActionTarget::Frontmatter { key: fm_key, .. } => {
            // read → set_frontmatter_field → atomic_write. Читаем СВЕЖИЙ on-disk (current_content),
            // round-trip-reject set_frontmatter_field — единственный санкционированный fm-писатель.
            let base = current_content.clone().unwrap_or_default();
            let value = action.value.clone().unwrap_or_default();
            match crate::parser::set_frontmatter_field(&base, fm_key, &value) {
                Ok(new_content) => {
                    let abs_w = abs.clone();
                    spawn_atomic_write(abs_w, new_content.into_bytes()).await
                }
                Err(e) => Err(format!("set_frontmatter_field: {e:?}")),
            }
        }
    };

    // ── РУБЕЖ 7: ledger FINISH (поглощающий) ──────────────────────────────────────────────────
    match write_result {
        Ok(()) => {
            let summary = success_summary(action, &rel);
            let _ = ledger
                .finish(&key, audit::STATE_EXECUTED, &summary, Some(undo.to_cols()))
                .await;
            ApplyOutcome::Executed { summary, undo }
        }
        Err(e) => {
            // Write упал ПОСЛЕ снапшота/ledger-строки: фиксируем Failed без undo (нечего откатывать —
            // atomic_write либо записал целиком, либо ничего; снапшот для create не делали).
            let _ = ledger.finish(&key, STATE_FAILED, &e, None).await;
            ApplyOutcome::Failed(e)
        }
    }
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
        ActionTarget::NoteCreate { .. } | ActionTarget::NoteEdit { .. } => {
            action.content.as_deref()
        }
        ActionTarget::Frontmatter { .. } => action.value.as_deref(),
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
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::vault::history::{list_snapshots, read_snapshot};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    /// Открывает временный vault + БД; возвращает (dir, canon_root, AuditSink). canon_root
    /// КАНОНИЗИРОВАН (предусловие resolve_vault_path_for_write — на macOS /tmp → /private/tmp).
    async fn setup() -> (TempDir, PathBuf, AuditSink) {
        let dir = TempDir::new().unwrap();
        let canon_root = dir.path().canonicalize().unwrap();
        let db = Database::open(canon_root.join(".nexus/nexus.db"))
            .await
            .unwrap();
        let sink = AuditSink::new(db.writer().clone(), db.reader().clone());
        // db дропнется в конце функции — но writer/reader клонированы в sink и переживут.
        // Держим db живой через утечку хэндлов: возвращаем sink (клон), db дропается, но актор
        // живёт пока жив хотя бы один клон writer. Для теста этого достаточно.
        std::mem::forget(db);
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

    /// Прочитать строку ledger по ключу (через reader sink'а напрямую — для проверок состояния).
    async fn ledger_row(sink: &AuditSink, key: &str) -> Option<super::super::audit::ActionRow> {
        super::super::audit::lookup(&sink.reader, key)
            .await
            .unwrap()
    }

    /// Восстановить ключ так же, как его построит apply (для проверки строки ledger).
    fn key_for(
        run_id: i64,
        action: &Action,
        on_disk: Option<&str>,
        planned: Option<&str>,
    ) -> String {
        let rel = action.target.rel();
        let payload = action_payload(action);
        let target_hash = on_disk
            .map(|c| crate::vault::content_hash(c.as_bytes()))
            .unwrap_or_else(|| crate::vault::content_hash(planned.unwrap_or("").as_bytes()));
        let args = canonical_args(Some(rel), payload);
        idempotency_key(run_id, action.target.tool_name(), &args, &target_hash)
    }

    /// CREATE: пишет новую заметку; ledger Executed; undo=Trash; re-create-over-existing fails-closed.
    #[tokio::test]
    async fn create_writes_note_ledger_executed_undo_trash() {
        let (_d, root, sink) = setup().await;
        let action = Action::note_create("Notes/New.md", "hello body");

        let outcome = apply_action(&action, 1, &root, &sink, None).await;
        match &outcome {
            ApplyOutcome::Executed { undo, .. } => {
                assert_eq!(
                    *undo,
                    UndoHandle::Trash {
                        trash_rel: "Notes/New.md".to_string()
                    },
                    "undo create = Trash"
                );
            }
            other => panic!("ожидался Executed, получено {other:?}"),
        }
        assert_eq!(read(&root, "Notes/New.md"), "hello body");

        // ledger: строка Executed с undo=trash.
        let key = key_for(1, &action, None, Some("hello body"));
        let row = ledger_row(&sink, &key).await.expect("ledger-строка есть");
        assert_eq!(row.state, "executed");
        assert!(row.outcome.is_some(), "outcome зафиксирован");
        assert_eq!(row.undo_kind.as_deref(), Some("trash"));

        // re-create поверх существующего → fails-closed (диск не тронут). run_id другой, чтобы не
        // упереться в идемпотентность того же ключа.
        let again = apply_action(&action, 2, &root, &sink, None).await;
        assert!(
            matches!(again, ApplyOutcome::Failed(_)),
            "create поверх существующего fails-closed, было {again:?}"
        );
        assert_eq!(
            read(&root, "Notes/New.md"),
            "hello body",
            "файл не перезаписан"
        );
    }

    /// EDIT: снапшот ПРЕД-edit контента (manual=true), перезапись; ledger Executed + undo=Snapshot{ts}.
    #[tokio::test]
    async fn edit_snapshots_pre_content_and_overwrites() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "Notes/E.md", "ORIGINAL");
        let action = Action::note_edit("Notes/E.md", "EDITED");

        let outcome = apply_action(&action, 1, &root, &sink, None).await;
        let undo = match &outcome {
            ApplyOutcome::Executed { undo, .. } => undo.clone(),
            other => panic!("ожидался Executed, получено {other:?}"),
        };
        // Содержимое перезаписано.
        assert_eq!(read(&root, "Notes/E.md"), "EDITED");
        // undo = Snapshot{ts}; снапшот держит ПРЕД-edit контент.
        let ts = match undo {
            UndoHandle::Snapshot { ts, rel } => {
                assert_eq!(rel, "Notes/E.md");
                ts as u64
            }
            other => panic!("ожидался Snapshot, получено {other:?}"),
        };
        assert_eq!(
            read_snapshot(&root, "Notes/E.md", ts).unwrap(),
            "ORIGINAL",
            "снапшот держит ПРЕД-edit содержимое (обратимость)"
        );

        let key = key_for(1, &action, Some("ORIGINAL"), None);
        let row = ledger_row(&sink, &key).await.expect("ledger-строка");
        assert_eq!(row.state, "executed");
        assert_eq!(row.undo_kind.as_deref(), Some("snapshot"));
        assert_eq!(row.undo_ref.as_deref(), Some(ts.to_string().as_str()));
    }

    /// KEYSTONE throttle-bypass: две быстрые правки подряд ОБЕ оставляют снапшот (manual=true бьёт
    /// 90с-троттл). С manual=false второй снапшот схлопнулся бы — тест бы упал.
    #[tokio::test]
    async fn rapid_edits_both_snapshot_throttle_bypass() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "R.md", "v0");

        // Правка 1: v0 → v1 (run 1).
        let a1 = Action::note_edit("R.md", "v1");
        assert!(matches!(
            apply_action(&a1, 1, &root, &sink, None).await,
            ApplyOutcome::Executed { .. }
        ));
        // Правка 2 СРАЗУ ЖЕ: v1 → v2 (run 2). В пределах 90с — авто-троттл бы пропустил снапшот v1.
        let a2 = Action::note_edit("R.md", "v2");
        assert!(matches!(
            apply_action(&a2, 2, &root, &sink, None).await,
            ApplyOutcome::Executed { .. }
        ));

        let snaps = list_snapshots(&root, "R.md").unwrap();
        assert_eq!(
            snaps.len(),
            2,
            "manual=true: обе быстрые правки оставили снапшот (троттл байпас); с manual=false было бы 1"
        );
        // Снапшоты держат ПРЕД-правочные контенты: v0 (перед 1-й) и v1 (перед 2-й).
        let mut contents: Vec<String> = snaps
            .iter()
            .map(|s| read_snapshot(&root, "R.md", s.ts).unwrap())
            .collect();
        contents.sort();
        assert_eq!(contents, vec!["v0".to_string(), "v1".to_string()]);
        assert_eq!(
            read(&root, "R.md"),
            "v2",
            "финальное содержимое — последняя правка"
        );
    }

    /// FRONTMATTER: через set_frontmatter_field; снапшот-перед; ledger Executed + undo=Snapshot.
    #[tokio::test]
    async fn set_frontmatter_writes_and_snapshots() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "F.md", "---\ntitle: T\n---\n\nbody\n");
        let action = Action::frontmatter("F.md", "status", "done");

        let outcome = apply_action(&action, 1, &root, &sink, None).await;
        let undo = match &outcome {
            ApplyOutcome::Executed { undo, .. } => undo.clone(),
            other => panic!("ожидался Executed, получено {other:?}"),
        };
        let new = read(&root, "F.md");
        assert!(new.contains("status: done"), "ключ записан: {new:?}");
        assert!(new.contains("title: T"), "остальной YAML сохранён");

        // Снапшот держит ПРЕД-правочный контент.
        let ts = match undo {
            UndoHandle::Snapshot { ts, .. } => ts as u64,
            other => panic!("ожидался Snapshot, получено {other:?}"),
        };
        assert_eq!(
            read_snapshot(&root, "F.md", ts).unwrap(),
            "---\ntitle: T\n---\n\nbody\n"
        );
    }

    /// SYMLINK RAMPART (критический security-тест): симлинк ВНУТРИ vault наружу. classify (лексический)
    /// видит Auto, НО resolve_vault_path_for_write резолвит родителя→цель НАРУЖИ → PathEscape, внешний
    /// файл НЕ тронут. Доказывает двух-рубежную защиту (lexical classify слеп к симлинку).
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_rampart_blocks_write_outside_vault() {
        use std::os::unix::fs::symlink;

        let (_d, root, sink) = setup().await;
        // Внешняя цель ВНЕ vault, с известным содержимым.
        let outside_dir = TempDir::new().unwrap();
        let outside_target = outside_dir.path().canonicalize().unwrap().join("secret.md");
        fs::write(&outside_target, "OUTSIDE-UNTOUCHED").unwrap();

        // Симлинк ВНУТРИ vault: vault/evil.md -> /…/outside/secret.md.
        symlink(&outside_target, abs_of(&root, "evil.md")).unwrap();

        // Лексический classify видит "evil.md" как обычное имя в корне → Auto (слеп к симлинку).
        let action = Action::note_edit("evil.md", "PWNED");
        let ctx = super::super::classify::ClassifyCtx {
            root: &root,
            overwrite_threshold: 64 * 1024,
        };
        assert_eq!(
            super::super::classify::classify(&action, &ctx),
            super::super::classify::RiskTier::Auto,
            "classify лексически слеп к симлинку — говорит Auto"
        );

        // НО apply: рубеж 1 видит leaf-симлинк (symlink_metadata) → PathEscape, НИ ОДНОГО write/чтения
        // по симлинку. (Родитель evil.md — vault-корень, ВНУТРИ; resolve канонизирует только родителя,
        // поэтому leaf-симлинк ловит ДОПОЛНИТЕЛЬНАЯ symlink_metadata-проверка рубежа 1.)
        let outcome = apply_action(&action, 1, &root, &sink, None).await;

        // Главный инвариант: внешний файл вне vault НЕ изменён.
        assert_eq!(
            fs::read_to_string(&outside_target).unwrap(),
            "OUTSIDE-UNTOUCHED",
            "симлинк-побег: внешний файл вне vault НЕ должен быть перезаписан"
        );
        // Сильное утверждение: apply отверг как PathEscape (рубеж, а не побочный эффект rename).
        assert_eq!(
            outcome,
            ApplyOutcome::PathEscape,
            "leaf-симлинк наружу → PathEscape (двух-рубежная защита), получено {outcome:?}"
        );
        // И симлинк НЕ заменён реальным файлом — мы вообще не писали.
        assert!(
            fs::symlink_metadata(abs_of(&root, "evil.md"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "симлинк не тронут (write не выполнялся)"
        );
    }

    /// SYMLINK-КАТАЛОГ наружу: `vault/dirlink -> /outside`, запись `vault/dirlink/x.md`. Здесь побег
    /// ловит САМ resolve_vault_path_for_write (канонизирует РОДИТЕЛЯ `dirlink` → наружу → не starts_with
    /// root). Дополняет leaf-кейс выше — обе формы симлинк-побега → PathEscape, внешний каталог нетронут.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_dir_rampart_blocks_write_outside_vault() {
        use std::os::unix::fs::symlink;
        let (_d, root, sink) = setup().await;
        let outside_dir = TempDir::new().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap();
        // Симлинк-КАТАЛОГ внутри vault → внешний каталог.
        symlink(&outside, abs_of(&root, "dirlink")).unwrap();

        // create в symlinked-каталог: classify лексически Auto (видит "dirlink/x.md" как обычный путь).
        let action = Action::note_create("dirlink/x.md", "PWNED");
        let outcome = apply_action(&action, 1, &root, &sink, None).await;
        assert_eq!(
            outcome,
            ApplyOutcome::PathEscape,
            "симлинк-каталог наружу → PathEscape, получено {outcome:?}"
        );
        assert!(
            !outside.join("x.md").exists(),
            "файл НЕ создан во внешнем каталоге"
        );
    }

    /// LEDGER write-before-act: строка (state=Executing) существует ДО/НЕЗАВИСИМО от файлового write.
    /// Проверяем через ручной record_before + проверку строки до finish.
    #[tokio::test]
    async fn ledger_row_exists_before_act() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "L.md", "before");
        let action = Action::note_edit("L.md", "after");

        // Ручной write-before-act (как сделает apply шагом 4), затем проверяем строку ДО любого write.
        let key = key_for(1, &action, Some("before"), None);
        let entry = ActionEntry {
            run_id: 1,
            idempotency_key: key.clone(),
            tool_name: action.target.tool_name().to_string(),
            target_rel: Some("L.md".to_string()),
            risk_tier: super::super::audit::TIER_AUTO.to_string(),
            state: STATE_EXECUTING.to_string(),
            content_hash: Some(crate::vault::content_hash("before".as_bytes())),
            diff_summary: None,
        };
        sink.record_before(entry).await.unwrap();

        // Строка есть, state=executing, outcome=NULL — ДО любого write. Файл всё ещё исходный.
        let row = ledger_row(&sink, &key).await.expect("строка-якорь");
        assert_eq!(row.state, "executing");
        assert!(row.outcome.is_none(), "outcome NULL до finish");
        assert_eq!(read(&root, "L.md"), "before", "файл ещё не тронут");

        // Симулируем краш сразу после write-before (НЕ вызываем finish) → строка CrashedMidExecute.
        match sink.replay_decision(&key).await.unwrap() {
            ReplayDecision::CrashedMidExecute(r) => {
                assert_eq!(
                    r.content_hash.as_deref(),
                    Some(crate::vault::content_hash("before".as_bytes()).as_str()),
                    "строка несёт content_hash для re-check (восстановима)"
                );
            }
            other => panic!("ожидался CrashedMidExecute, получено {other:?}"),
        }
    }

    /// REPLAY AlreadyDone: то же действие дважды (тот же run_id/key) → второй вызов AlreadyDone, файл
    /// записан РОВНО один раз (no double-write).
    #[tokio::test]
    async fn replay_already_done_no_double_write() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "D.md", "orig");
        let action = Action::note_edit("D.md", "v1");

        let first = apply_action(&action, 1, &root, &sink, None).await;
        assert!(matches!(first, ApplyOutcome::Executed { .. }));
        assert_eq!(read(&root, "D.md"), "v1");

        // Второй идентичный вызов (тот же run_id ⇒ тот же ключ; и тот же on-disk hash на момент 2-го
        // classify? — нет: файл теперь "v1", а ключ строится из on-disk hash. Чтобы воспроизвести
        // ИМЕННО replay-ветку, ключ должен совпасть → действие должно стартовать с тем же on-disk
        // состоянием. Здесь после 1-й записи on-disk="v1", target_hash другой → НОВЫЙ ключ → это уже
        // НЕ дубль, а новое действие "v1→v1". Поэтому для AlreadyDone-ветки берём create (target_hash
        // от planned content, стабилен).
        let create = Action::note_create("New2.md", "fixed");
        let c1 = apply_action(&create, 5, &root, &sink, None).await;
        assert!(matches!(c1, ApplyOutcome::Executed { .. }));
        let first_content = read(&root, "New2.md");

        // Тот же create (run 5, тот же ключ) повторно: цель теперь существует, НО ключ совпал →
        // ledger record_before отобьётся UNIQUE → replay → AlreadyDone (НЕ повторная запись/ошибка).
        let c2 = apply_action(&create, 5, &root, &sink, None).await;
        assert!(
            matches!(c2, ApplyOutcome::AlreadyDone(_)),
            "повтор того же действия → AlreadyDone, было {c2:?}"
        );
        assert_eq!(
            read(&root, "New2.md"),
            first_content,
            "файл не переписан повторно"
        );
    }

    /// REPLAY CrashedMidExecute + on-disk drift → Failed (no clobber). Берём CREATE: его ключ НЕ зависит
    /// от on-disk hash (target_hash = отпечаток ПЛАНИРУЕМОГО контента), поэтому ключ СТАБИЛЕН при дрейфе
    /// диска — что и нужно, чтобы попасть в CrashedMidExecute-ветку (у edit дрейф диска «уводит» ключ →
    /// Fresh, а не replay; для edit/frontmatter ранний дрейф ловит Рубеж 3 по classify_hash). Симулируем
    /// краш: вставляем строку write-before (outcome=NULL) с content_hash от «состояния на момент попытки»,
    /// затем на диске ИНОЕ содержимое (дрейф), затем apply тем же ключом → CrashedMidExecute → re-check
    /// hash ≠ → Failed, диск не тронут.
    #[tokio::test]
    async fn replay_crashed_with_drift_fails_no_clobber() {
        let (_d, root, sink) = setup().await;
        let action = Action::note_create("C.md", "fixed-planned");

        // Ключ create (classify_hash=None) = blake3(.., target_hash=hash("fixed-planned")) — стабилен.
        let key = key_for(1, &action, None, Some("fixed-planned"));
        // Строка-якорь крашнутой попытки: content_hash от «того, что было на диске на момент попытки».
        let entry = ActionEntry {
            run_id: 1,
            idempotency_key: key.clone(),
            tool_name: action.target.tool_name().to_string(),
            target_rel: Some("C.md".to_string()),
            risk_tier: super::super::audit::TIER_AUTO.to_string(),
            state: STATE_EXECUTING.to_string(),
            content_hash: Some(crate::vault::content_hash("at-attempt-state".as_bytes())),
            diff_summary: None,
        };
        sink.record_before(entry).await.unwrap(); // outcome=NULL → CrashedMidExecute при replay

        // На диске ИНОЕ содержимое (дрейф vs то, что зафиксировала крашнутая строка).
        write_existing(&root, "C.md", "DRIFTED-EXTERNALLY");

        // apply тем же ключом: record_before отобьётся UNIQUE → replay CrashedMidExecute → re-check
        // on-disk hash (от "DRIFTED…") ≠ row.content_hash (от "at-attempt-state") → Failed, НЕ клобберим.
        let outcome = apply_action(&action, 1, &root, &sink, None).await;
        assert!(
            matches!(outcome, ApplyOutcome::Failed(_)),
            "drift при CrashedMidExecute → Failed, было {outcome:?}"
        );
        assert_eq!(
            read(&root, "C.md"),
            "DRIFTED-EXTERNALLY",
            "содержимое на диске НЕ затёрто (no clobber)"
        );
        // Строка теперь терминальна (Failed) — повторный finish не нужен.
        let row = ledger_row(&sink, &key).await.unwrap();
        assert_eq!(row.state, "failed");
        assert!(row.outcome.is_some());
    }

    /// REPLAY CrashedMidExecute БЕЗ дрейфа → complete-forward: re-check hash совпал → доводим write.
    /// CREATE: вставляем крашнутую строку с content_hash, совпадающим с тем, что на диске СЕЙЧАС, затем
    /// apply тем же ключом → CrashedMidExecute → hash match → запись доводится, строка → Executed.
    #[tokio::test]
    async fn replay_crashed_no_drift_completes_forward() {
        let (_d, root, sink) = setup().await;
        let action = Action::note_create("CF.md", "planned-body");
        let key = key_for(1, &action, None, Some("planned-body"));

        // На диске уже лежит «состояние на момент крашнутой попытки» (как будто write частично/повторно).
        write_existing(&root, "CF.md", "partial-on-disk");
        let entry = ActionEntry {
            run_id: 1,
            idempotency_key: key.clone(),
            tool_name: action.target.tool_name().to_string(),
            target_rel: Some("CF.md".to_string()),
            risk_tier: super::super::audit::TIER_AUTO.to_string(),
            state: STATE_EXECUTING.to_string(),
            // content_hash СОВПАДАЕТ с on-disk сейчас → нет дрейфа → complete-forward.
            content_hash: Some(crate::vault::content_hash("partial-on-disk".as_bytes())),
            diff_summary: None,
        };
        sink.record_before(entry).await.unwrap();

        let outcome = apply_action(&action, 1, &root, &sink, None).await;
        assert!(
            matches!(outcome, ApplyOutcome::Executed { .. }),
            "CrashedMidExecute без дрейфа → complete-forward (Executed), было {outcome:?}"
        );
        assert_eq!(read(&root, "CF.md"), "planned-body", "запись доведена");
        let row = ledger_row(&sink, &key).await.unwrap();
        assert_eq!(row.state, "executed");
    }

    /// DRIFT при classify_hash: at-classify hash отличается от on-disk сейчас → Failed (не клобберим),
    /// файл не тронут — прямой drift-рубеж (без replay-ветки).
    #[tokio::test]
    async fn classify_hash_drift_aborts() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "G.md", "current-on-disk");
        let action = Action::note_edit("G.md", "new");

        // classify_hash от УСТАРЕВШЕГО состояния (не совпадает с on-disk "current-on-disk").
        let stale = crate::vault::content_hash("stale-view".as_bytes());
        let outcome = apply_action(&action, 1, &root, &sink, Some(&stale)).await;
        assert!(
            matches!(outcome, ApplyOutcome::Failed(_)),
            "stale classify_hash → Failed, было {outcome:?}"
        );
        assert_eq!(
            read(&root, "G.md"),
            "current-on-disk",
            "файл не тронут при дрейфе"
        );

        // А при СОВПАДАЮЩЕМ classify_hash — проходит.
        let fresh = crate::vault::content_hash("current-on-disk".as_bytes());
        let ok = apply_action(&action, 2, &root, &sink, Some(&fresh)).await;
        assert!(
            matches!(ok, ApplyOutcome::Executed { .. }),
            "свежий hash → Executed"
        );
        assert_eq!(read(&root, "G.md"), "new");
    }

    /// FIX 1 — симлинк-безопасный create_dir_all: предсуществующий компонент-каталог родителя — симлинк
    /// НАРУЖУ vault (`vault/sub -> /outside`); create на ГЛУБОКИЙ путь `sub/a/b/new.md`. Без проверки
    /// create_dir_all создал бы `/outside/a/b` (пустые каталоги ВНЕ vault) ДО конфайнмент-reject. С Fix 1
    /// reject_symlinked_components ловит `sub`-симлинк ДО create_dir_all → PathEscape, ничего не создано
    /// под /outside. Доказывает: НЕТ создания каталогов вне vault через симлинк-компонент.
    #[cfg(unix)]
    #[tokio::test]
    async fn create_dir_all_symlink_component_no_outside_dirs() {
        use std::os::unix::fs::symlink;
        let (_d, root, sink) = setup().await;
        let outside_dir = TempDir::new().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap();
        // Предсуществующий компонент-СИМЛИНК наружу: vault/sub -> /outside (каталог).
        symlink(&outside, abs_of(&root, "sub")).unwrap();

        // create на глубокий путь СКВОЗЬ симлинк-компонент: sub/a/b — ещё не существуют.
        let action = Action::note_create("sub/a/b/new.md", "PWNED");
        let outcome = apply_action(&action, 1, &root, &sink, None).await;

        // Главный инвариант: НИ ОДНОГО каталога не создано под /outside.
        assert!(
            !outside.join("a").exists(),
            "Fix 1: create_dir_all НЕ должен создавать каталоги ВНЕ vault сквозь симлинк-компонент"
        );
        assert!(
            !outside.join("a/b").exists(),
            "глубже тоже ничего не создано"
        );
        assert!(
            !outside.join("a/b/new.md").exists(),
            "файл вне vault не создан"
        );
        // И apply отверг как PathEscape (рубеж, а не побочный rename).
        assert_eq!(
            outcome,
            ApplyOutcome::PathEscape,
            "симлинк-компонент родителя наружу → PathEscape, получено {outcome:?}"
        );
    }

    /// FIX 2 (focused unit) — ре-рид-перед-записью: helper детектит дрейф on-disk vs ожидаемый хеш.
    /// Это БЕЗУСЛОВНЫЙ фенс (не зависит от classify_hash). Доказывает: совпадение → НЕ дрейф (запись
    /// разрешена), внешняя правка → дрейф (запись отменяется), удаление файла → дрейф.
    #[tokio::test]
    async fn reread_drift_helper_detects_external_change() {
        let (_d, root, _sink) = setup().await;
        write_existing(&root, "RR.md", "BASE");
        let abs = abs_of(&root, "RR.md");
        let base_hash = crate::vault::content_hash("BASE".as_bytes());

        // Совпадение: диск == то, что прочитали в Рубеже 2 → НЕ дрейф (запись пойдёт).
        assert!(
            !reread_drift_detected(&abs, Some(&base_hash)).await,
            "без внешней правки дрейфа нет"
        );

        // Внешняя правка в окне read→write: диск изменился → ДРЕЙФ (запись должна отмениться).
        fs::write(&abs, "EXTERNALLY-CHANGED").unwrap();
        assert!(
            reread_drift_detected(&abs, Some(&base_hash)).await,
            "внешняя правка → дрейф (re-read fence срабатывает безусловно)"
        );

        // Внешнее удаление: fresh=None vs Some(expected) → ДРЕЙФ (не пишем поверх гонки удаления).
        fs::remove_file(&abs).unwrap();
        assert!(
            reread_drift_detected(&abs, Some(&base_hash)).await,
            "внешнее удаление → дрейф"
        );
    }

    /// FIX 2 (no-false-positive regression) — стабильный диск: re-read фенс НЕ должен ложно блокировать
    /// легитимную правку (между внутренним read Рубежа 2 и ре-ридом диск не менялся → хеши совпадают →
    /// запись проходит). Дополняет drift-ветку helper-теста выше: вместе они фиксируют, что фенс ловит
    /// ИМЕННО дрейф и не калечит нормальный overwrite.
    #[tokio::test]
    async fn reread_fence_allows_legitimate_overwrite() {
        let (_d, root, sink) = setup().await;
        write_existing(&root, "OK.md", "STABLE");
        let action = Action::note_edit("OK.md", "NEW-CONTENT");
        let outcome = apply_action(&action, 1, &root, &sink, None).await;
        assert!(
            matches!(outcome, ApplyOutcome::Executed { .. }),
            "стабильный диск: re-read фенс НЕ должен ложно блокировать легитимную правку, было {outcome:?}"
        );
        assert_eq!(
            read(&root, "OK.md"),
            "NEW-CONTENT",
            "легитимная правка записана"
        );
    }

    /// FIX 3 (cfg unix) — hardlink-reject на overwrite: ВНУТРИ vault создаём ХАРДЛИНК на ВНЕШНИЙ файл
    /// (`vault/hard.md` == inode внешнего secret.md). symlink_metadata().is_symlink() для хардлинка ЛОЖЕН,
    /// поэтому leaf-симлинк-проверка его НЕ ловит. Fix 3 сверяет nlink>1 → PathEscape: внешний файл НЕ
    /// затронут, его содержимое НЕ утекает в снапшот. Доказывает defense-in-depth против info-leak.
    #[cfg(unix)]
    #[tokio::test]
    async fn hardlink_overwrite_rejected_no_outside_leak() {
        let (_d, root, sink) = setup().await;
        let outside_dir = TempDir::new().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap().join("secret.md");
        fs::write(&outside, "OUTSIDE-SECRET").unwrap();

        // Хардлинк ВНУТРИ vault на ВНЕШНИЙ inode: vault/hard.md → тот же inode, что outside/secret.md.
        // (hard_link следует обычной семантике: оба имени делят inode, nlink=2.)
        fs::hard_link(&outside, abs_of(&root, "hard.md")).unwrap();
        // Sanity: это НЕ симлинк (иначе ловила бы leaf-проверка, а не Fix 3).
        assert!(
            !fs::symlink_metadata(abs_of(&root, "hard.md"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "хардлинк — не симлинк (иначе тест проверял бы не ту гарду)"
        );

        let action = Action::note_edit("hard.md", "PWNED-THROUGH-HARDLINK");
        let outcome = apply_action(&action, 1, &root, &sink, None).await;

        // Внешний файл (тот же inode) НЕ изменён.
        assert_eq!(
            fs::read_to_string(&outside).unwrap(),
            "OUTSIDE-SECRET",
            "Fix 3: внешний файл за хардлинком НЕ должен быть перезаписан"
        );
        // apply отверг как PathEscape (nlink>1), а не как побочный эффект.
        assert_eq!(
            outcome,
            ApplyOutcome::PathEscape,
            "хардлинк (nlink>1) на overwrite → PathEscape, получено {outcome:?}"
        );
        // Снапшот внешнего содержимого НЕ создан (чтения по хардлинку не было до reject).
        let snaps = list_snapshots(&root, "hard.md").unwrap();
        assert!(
            snaps.is_empty(),
            "info-leak: НЕ должно быть снапшота внешнего содержимого, есть {snaps:?}"
        );
    }
}
