//! Idempotency-ledger актуатора (AGENT-3b) — async-CRUD над `agent_actions` (миграция 022).
//!
//! Журнал КАЖДОГО действия актуатора внутри прогона: write-before-act основа + якорь идемпотентного
//! replay. Все мутации через единственный [`WriteActor`] (ADR-003 — сериализованы); чтения через
//! [`ReadPool`]. Append/update-only: строки НЕ удаляются (журнал подотчётности); меняются только
//! state/outcome/undo/updated_at.
//!
//! ## Контракт replay — ветвление по ПРИСУТСТВИЮ outcome, НЕ по присутствию ключа
//! Это центральный инвариант слоя (ADR-009): [`replay_decision`] решает по тому, ЕСТЬ ЛИ `outcome`, а
//! НЕ по тому, есть ли строка с таким ключом:
//!
//! - ключа нет → [`ReplayDecision::Fresh`] — свежее действие, исполнять;
//! - ключ есть, `outcome` IS NULL → [`ReplayDecision::CrashedMidExecute`] — упали МЕЖДУ write-before и
//!   фиксацией исхода; вызывающий (AGENT-3c) пере-проверит on-disk `content_hash` (оптимистичная
//!   конкуренция) и решит безопасный повтор/пропуск;
//! - ключ есть, `outcome` NOT NULL → [`ReplayDecision::AlreadyDone`] — вернуть записанный исход, НЕ
//!   повторять побочный эффект.
//!
//! Почему НЕ ключ: ключ ПРИСУТСТВУЕТ в обоих терминальных и крашнутых случаях (его пишет `record_before`
//! ДО эффекта). Если ветвиться «ключ есть ⇒ done», крашнутое-на-середине действие посчиталось бы
//! завершённым и его эффект потерялся бы навсегда (или, хуже, дубль не был бы детектирован для re-check).
//! Терминальность определяет ТОЛЬКО присутствие `outcome` (его ставит [`finish`]).
//!
//! ## idempotency_key = blake3(run_id, tool_name, canonical_args, target_hash@classify)
//! UNIQUE-фенс ([`idempotency_key`]): два идентичных действия одного прогона дают ОДИН ключ → второй
//! INSERT отбивается UNIQUE → caller делает [`lookup`]/[`replay_decision`]. `target_hash` фиксируется НА
//! МОМЕНТ classify (часть ключа); on-disk `content_hash` хранится отдельной колонкой как токен
//! оптимистичной конкуренции для re-check в 3c.

use rusqlite::{params, OptionalExtension};

use crate::db::{DbResult, ReadPool, WriteActor};
use crate::scheduler::now_secs;

/// Имена состояний статус-машины (значения `agent_actions.state`) — единый источник со
/// [`super::ActionState`] (см. [`super::ActionState::as_str`]). Строковые литералы держим рядом с SQL,
/// чтобы не разъехались по опечаткам.
pub const STATE_CLASSIFIED: &str = "classified";
pub const STATE_EXECUTING: &str = "executing";
pub const STATE_EXECUTED: &str = "executed";
pub const STATE_FAILED: &str = "failed";

/// Дискриминанты тира риска (значения `agent_actions.risk_tier`) — зеркало [`super::classify::RiskTier::as_str`].
pub const TIER_AUTO: &str = "auto";
pub const TIER_CONFIRM: &str = "confirm";
pub const TIER_HARDBLOCKED: &str = "hardblocked";

/// Параметры вставки строки действия (write-before-act). `outcome` НЕ передаётся — он стартует NULL и
/// ставится только [`finish`] (присутствие outcome — ветка replay).
#[derive(Debug, Clone)]
pub struct ActionEntry {
    pub run_id: i64,
    pub idempotency_key: String,
    pub tool_name: String,
    pub target_rel: Option<String>,
    pub risk_tier: String,
    /// Начальное состояние (обычно `executing` для write-before-act, либо `classified`).
    pub state: String,
    /// on-disk hash цели на момент classify (токен оптимистичной конкуренции). None — у действий без файла.
    pub content_hash: Option<String>,
    /// Усечённое резюме диффа (приватность; AGENT-6 ужесточит). None пока нет.
    pub diff_summary: Option<String>,
}

/// Снимок строки `agent_actions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRow {
    pub id: i64,
    pub run_id: i64,
    pub idempotency_key: String,
    pub tool_name: String,
    pub target_rel: Option<String>,
    pub risk_tier: String,
    pub state: String,
    pub content_hash: Option<String>,
    pub undo_kind: Option<String>,
    pub undo_ref: Option<String>,
    pub outcome: Option<String>,
    pub diff_summary: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ActionRow {
    /// Терминальна ли строка для replay — по ПРИСУТСТВИЮ `outcome`, НЕ по `state`. `finish` ставит
    /// outcome атомарно с терминальным state; до этого (даже в state='executing') строка НЕ терминальна.
    pub fn is_terminal(&self) -> bool {
        self.outcome.is_some()
    }
}

/// Решение replay-проверки (см. модульный контракт). Ветвится по ПРИСУТСТВИЮ outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayDecision {
    /// Ключа нет — свежее действие, исполнять.
    Fresh,
    /// Ключ есть и `outcome` зафиксирован — действие уже завершено; вернуть записанный исход, не повторять.
    AlreadyDone(String),
    /// Ключ есть, но `outcome` NULL — крах между write-before и фиксацией исхода; вызывающий (3c)
    /// пере-проверит on-disk content_hash и решит повтор/пропуск. Несёт всю строку для этого re-check.
    CrashedMidExecute(Box<ActionRow>),
}

/// Сериализация UndoHandle в (kind, ref) для хранения в ledger. Зеркало в [`super::UndoHandle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoCols {
    pub kind: String,
    pub reference: String,
}

/// Стабильная канонизация аргументов действия для idempotency_key. КРИТИЧНО детерминирована: тот же
/// логический аргумент-набор всегда даёт ту же строку (иначе ключ «плавает» и replay не сработает).
/// Формат — позиционный с разделителем `\u{1f}` (Unit Separator, не встречается в путях/значениях),
/// каждое поле с префиксом-тегом. None кодируется как `-` (отличимо от пустой строки `s:`).
pub fn canonical_args(target_rel: Option<&str>, payload: Option<&str>) -> String {
    fn field(tag: char, v: Option<&str>) -> String {
        match v {
            Some(s) => format!("{tag}s:{s}"),
            None => format!("{tag}-"),
        }
    }
    // \u{1f} (US) как разделитель — детерминированно и не коллизирует с обычным текстом.
    format!("{}\u{1f}{}", field('r', target_rel), field('p', payload))
}

/// `idempotency_key = blake3(run_id, tool_name, canonical_args, target_hash@classify)`.
///
/// Все компоненты сворачиваются в ОДНУ строку с разделителем `\u{1f}` и хэшируются blake3 (стабилен
/// между платформами/версиями Rust — в отличие от `DefaultHasher`/SipHash с рандом-сидом). Ключ
/// МЕНЯЕТСЯ при изменении ЛЮБОГО компонента (run_id/tool/args/target_hash) и СТАБИЛЕН при тех же.
/// `target_hash` — отпечаток цели на момент classify (часть тождества действия): то же действие по уже
/// изменившейся цели даёт ДРУГОЙ ключ (не считается дублем — корректно, цель иная).
pub fn idempotency_key(
    run_id: i64,
    tool_name: &str,
    canonical_args: &str,
    target_hash: &str,
) -> String {
    let material = format!("{run_id}\u{1f}{tool_name}\u{1f}{canonical_args}\u{1f}{target_hash}");
    blake3::hash(material.as_bytes()).to_hex().to_string()
}

/// Записывает строку действия ПЕРЕД эффектом (write-before-act). INSERT с `outcome=NULL`; UNIQUE
/// `idempotency_key` — фенс: дубль действия отобьётся ошибкой UNIQUE (caller тогда делает
/// [`replay_decision`]). Возвращает `id` вставленной строки.
///
/// NB (AGENT-3b): здесь ТОЛЬКО API + жизненный цикл строки. Реальное УПОРЯДОЧИВАНИЕ этой записи
/// относительно дискового write — забота AGENT-3c (apply); здесь побочных эффектов на диск НЕТ.
pub async fn record_before(writer: &WriteActor, entry: ActionEntry) -> DbResult<i64> {
    writer
        .transaction(move |tx| {
            let ts = now_secs();
            tx.execute(
                "INSERT INTO agent_actions\
                 (run_id,idempotency_key,tool_name,target_rel,risk_tier,state,content_hash,diff_summary,outcome,created_at,updated_at) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9,?9)",
                params![
                    entry.run_id,
                    entry.idempotency_key,
                    entry.tool_name,
                    entry.target_rel,
                    entry.risk_tier,
                    entry.state,
                    entry.content_hash,
                    entry.diff_summary,
                    ts,
                ],
            )?;
            Ok(tx.last_insert_rowid())
        })
        .await
}

/// Терминирует действие: ставит финальный `state` + `outcome` (+ опц. UndoHandle) + бамп `updated_at`.
/// **ПОГЛОЩАЮЩИЙ:** если у строки УЖЕ есть `outcome` (терминальна), finish — no-op (первый терминал
/// побеждает; повторный handle/replay НЕ перезаписывает исход). Фенс — `WHERE outcome IS NULL` (по
/// присутствию outcome, согласовано с replay-контрактом — НЕ по state). Возвращает `true`, если строка
/// реально терминирована этим вызовом.
pub async fn finish(
    writer: &WriteActor,
    key: &str,
    state: &str,
    outcome: &str,
    undo: Option<UndoCols>,
) -> DbResult<bool> {
    let (key, state, outcome) = (key.to_string(), state.to_string(), outcome.to_string());
    let (undo_kind, undo_ref) = match undo {
        Some(u) => (Some(u.kind), Some(u.reference)),
        None => (None, None),
    };
    writer
        .transaction(move |tx| {
            let n = tx.execute(
                "UPDATE agent_actions SET state=?2, outcome=?3, undo_kind=?4, undo_ref=?5, updated_at=?6 \
                 WHERE idempotency_key=?1 AND outcome IS NULL",
                params![key, state, outcome, undo_kind, undo_ref, now_secs()],
            )?;
            Ok(n > 0)
        })
        .await
}

/// Читает строку действия по idempotency_key (`None` — нет такой). Это и есть replay-check на уровне
/// хранилища; [`replay_decision`] оборачивает его в ветвление по outcome.
pub async fn lookup(reader: &ReadPool, key: &str) -> DbResult<Option<ActionRow>> {
    let key = key.to_string();
    reader
        .query(move |c| {
            c.query_row(
                "SELECT id,run_id,idempotency_key,tool_name,target_rel,risk_tier,state,content_hash,\
                 undo_kind,undo_ref,outcome,diff_summary,created_at,updated_at \
                 FROM agent_actions WHERE idempotency_key=?1",
                [key],
                row_to_action,
            )
            .optional()
        })
        .await
}

/// Replay-решение по ключу — ВЕТВЛЕНИЕ ПО ПРИСУТСТВИЮ `outcome`, НЕ по присутствию ключа (см. модульный
/// контракт). Нет строки → [`ReplayDecision::Fresh`]; есть + outcome → [`ReplayDecision::AlreadyDone`];
/// есть + outcome NULL → [`ReplayDecision::CrashedMidExecute`].
pub async fn replay_decision(reader: &ReadPool, key: &str) -> DbResult<ReplayDecision> {
    Ok(match lookup(reader, key).await? {
        None => ReplayDecision::Fresh,
        Some(row) => match row.outcome.clone() {
            Some(outcome) => ReplayDecision::AlreadyDone(outcome),
            None => ReplayDecision::CrashedMidExecute(Box::new(row)),
        },
    })
}

/// Маппинг строки результата в [`ActionRow`] (порядок колонок фиксирован SELECT'ом в [`lookup`]).
fn row_to_action(r: &rusqlite::Row<'_>) -> rusqlite::Result<ActionRow> {
    Ok(ActionRow {
        id: r.get(0)?,
        run_id: r.get(1)?,
        idempotency_key: r.get(2)?,
        tool_name: r.get(3)?,
        target_rel: r.get(4)?,
        risk_tier: r.get(5)?,
        state: r.get(6)?,
        content_hash: r.get(7)?,
        undo_kind: r.get(8)?,
        undo_ref: r.get(9)?,
        outcome: r.get(10)?,
        diff_summary: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::TempDir;

    async fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .unwrap();
        (dir, db)
    }

    fn entry(run_id: i64, key: &str) -> ActionEntry {
        ActionEntry {
            run_id,
            idempotency_key: key.to_string(),
            tool_name: "note_edit".to_string(),
            target_rel: Some("Notes/N.md".to_string()),
            risk_tier: TIER_AUTO.to_string(),
            state: STATE_EXECUTING.to_string(),
            content_hash: Some("hash-at-classify".to_string()),
            diff_summary: Some("+1 -0".to_string()),
        }
    }

    /// record_before вставляет строку с outcome=NULL; lookup её возвращает; поля сохранены.
    #[tokio::test]
    async fn record_before_inserts_with_null_outcome() {
        let (_d, db) = open().await;
        let id = record_before(db.writer(), entry(1, "k1")).await.unwrap();
        assert!(id > 0);
        let row = lookup(db.reader(), "k1").await.unwrap().expect("вставлена");
        assert_eq!(row.run_id, 1);
        assert_eq!(row.tool_name, "note_edit");
        assert_eq!(row.target_rel.as_deref(), Some("Notes/N.md"));
        assert_eq!(row.risk_tier, TIER_AUTO);
        assert_eq!(row.state, STATE_EXECUTING);
        assert_eq!(row.content_hash.as_deref(), Some("hash-at-classify"));
        assert!(row.outcome.is_none(), "outcome стартует NULL");
        assert!(!row.is_terminal(), "без outcome — не терминальна");
    }

    /// UNIQUE idempotency_key: второй INSERT с тем же ключом — ОШИБКА (фенс дубля).
    #[tokio::test]
    async fn duplicate_key_is_rejected() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(1, "dup")).await.unwrap();
        let second = record_before(db.writer(), entry(1, "dup")).await;
        assert!(second.is_err(), "дубль idempotency_key отбит UNIQUE");
        // Строка одна.
        let row = lookup(db.reader(), "dup").await.unwrap().unwrap();
        assert!(row.outcome.is_none(), "первая строка не тронута");
    }

    /// finish ставит терминальный state+outcome; ПОГЛОЩАЮЩИЙ — второй finish с другим исходом no-op.
    #[tokio::test]
    async fn finish_is_absorbing() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(1, "k")).await.unwrap();

        assert!(
            finish(db.writer(), "k", STATE_EXECUTED, "первый", None)
                .await
                .unwrap(),
            "первый finish терминирует"
        );
        let row = lookup(db.reader(), "k").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_EXECUTED);
        assert_eq!(row.outcome.as_deref(), Some("первый"));
        assert!(row.is_terminal());

        // Второй finish с ДРУГИМ исходом/state — no-op, первый побеждает.
        assert!(
            !finish(db.writer(), "k", STATE_FAILED, "второй", None)
                .await
                .unwrap(),
            "поглощающий: повторный finish — no-op"
        );
        let row = lookup(db.reader(), "k").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_EXECUTED, "state первого финала");
        assert_eq!(
            row.outcome.as_deref(),
            Some("первый"),
            "исход первого финала"
        );
    }

    /// finish сохраняет UndoHandle (kind+ref).
    #[tokio::test]
    async fn finish_persists_undo() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(1, "u")).await.unwrap();
        let undo = UndoCols {
            kind: "snapshot".to_string(),
            reference: "1700000000".to_string(),
        };
        finish(db.writer(), "u", STATE_EXECUTED, "ok", Some(undo))
            .await
            .unwrap();
        let row = lookup(db.reader(), "u").await.unwrap().unwrap();
        assert_eq!(row.undo_kind.as_deref(), Some("snapshot"));
        assert_eq!(row.undo_ref.as_deref(), Some("1700000000"));
    }

    /// replay_decision: ключа нет → Fresh.
    #[tokio::test]
    async fn replay_fresh_when_absent() {
        let (_d, db) = open().await;
        assert_eq!(
            replay_decision(db.reader(), "nope").await.unwrap(),
            ReplayDecision::Fresh
        );
    }

    /// replay_decision: ключ есть, outcome зафиксирован → AlreadyDone(outcome). (Ветка по ПРИСУТСТВИЮ
    /// outcome, не ключа.)
    #[tokio::test]
    async fn replay_already_done_when_outcome_present() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(1, "done")).await.unwrap();
        finish(db.writer(), "done", STATE_EXECUTED, "результат", None)
            .await
            .unwrap();
        assert_eq!(
            replay_decision(db.reader(), "done").await.unwrap(),
            ReplayDecision::AlreadyDone("результат".to_string())
        );
    }

    /// replay_decision: ключ ЕСТЬ, но outcome NULL (краш между write-before и finish) →
    /// CrashedMidExecute, НЕ AlreadyDone. Это и есть «ветвление по outcome, НЕ по ключу»: ключ
    /// присутствует в ОБОИХ случаях — отличает их только наличие outcome.
    #[tokio::test]
    async fn replay_crashed_mid_execute_when_outcome_null() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(7, "crash")).await.unwrap();
        // НЕ вызываем finish — имитируем краш сразу после write-before.
        match replay_decision(db.reader(), "crash").await.unwrap() {
            ReplayDecision::CrashedMidExecute(row) => {
                assert_eq!(row.run_id, 7);
                assert!(row.outcome.is_none(), "outcome всё ещё NULL");
                assert_eq!(
                    row.content_hash.as_deref(),
                    Some("hash-at-classify"),
                    "несёт content_hash для re-check в 3c"
                );
            }
            other => panic!("ожидался CrashedMidExecute, получено {other:?}"),
        }
    }

    /// idempotency_key СТАБИЛЕН для одинаковых (run_id, tool, args, target_hash) и РАЗЛИЧАЕТСЯ при
    /// изменении ЛЮБОГО компонента.
    #[test]
    fn idempotency_key_stable_and_sensitive() {
        let args = canonical_args(Some("Notes/N.md"), Some("body"));
        let base = idempotency_key(1, "note_edit", &args, "th");

        // Стабильность: тот же вход → тот же ключ.
        assert_eq!(base, idempotency_key(1, "note_edit", &args, "th"));

        // Чувствительность к каждому компоненту.
        assert_ne!(base, idempotency_key(2, "note_edit", &args, "th"), "run_id");
        assert_ne!(
            base,
            idempotency_key(1, "note_create", &args, "th"),
            "tool_name"
        );
        assert_ne!(
            base,
            idempotency_key(
                1,
                "note_edit",
                &canonical_args(Some("Notes/Other.md"), Some("body")),
                "th"
            ),
            "args (rel)"
        );
        assert_ne!(
            base,
            idempotency_key(
                1,
                "note_edit",
                &canonical_args(Some("Notes/N.md"), Some("other")),
                "th"
            ),
            "args (payload)"
        );
        assert_ne!(
            base,
            idempotency_key(1, "note_edit", &args, "th2"),
            "target_hash"
        );
    }

    /// canonical_args различает None и пустую строку (Some("")) — иначе «нет значения» и «пустое
    /// значение» дали бы один ключ (коллизия тождества).
    #[test]
    fn canonical_args_distinguishes_none_from_empty() {
        assert_ne!(
            canonical_args(Some(""), None),
            canonical_args(None, None),
            "Some(\"\") != None для rel"
        );
        assert_ne!(
            canonical_args(Some("x"), Some("")),
            canonical_args(Some("x"), None),
            "Some(\"\") != None для payload"
        );
    }

    /// Индекс по run_id присутствует (выборка действий прогона — горячий путь).
    #[tokio::test]
    async fn run_index_present() {
        let (_d, db) = open().await;
        let n: i64 = db
            .reader()
            .query(|c| {
                c.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_agent_actions_run'",
                    [],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(n, 1, "idx_agent_actions_run создан миграцией 022");
    }
}
