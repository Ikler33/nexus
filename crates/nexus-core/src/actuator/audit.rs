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
/// Тир Confirm показан/предложен — батч ждёт решения (AGENT-3d). Строка `proposed` ещё НЕ терминальна
/// (outcome NULL): её решает [`transition`] (→approved) или [`finish`] (→rejected с исходом).
pub const STATE_PROPOSED: &str = "proposed";
/// Предложение одобрено — действие разрешено к apply (AGENT-3d). Промежуточное (outcome NULL),
/// дальше apply пишет СВОЮ строку executing→executed.
pub const STATE_APPROVED: &str = "approved";
/// Предложение отклонено — apply НЕ выполняется (AGENT-3d). Терминально (finish ставит outcome).
pub const STATE_REJECTED: &str = "rejected";
pub const STATE_EXECUTING: &str = "executing";
pub const STATE_EXECUTED: &str = "executed";
pub const STATE_FAILED: &str = "failed";
/// Успешное действие ОТКАЧЕНО (AGENT-4): снапшот восстановлен / created-файл перенесён в корзину. Ставится
/// [`mark_undone`] переходом `executed → undone`; [`actions_for_undo`] таких строк уже НЕ возвращает →
/// повторный `undo_run` их пропускает (идемпотентность отката). НЕ трогает `outcome` (он зафиксирован при
/// executed) — undo меняет только `state` (запись остаётся аудируемой).
pub const STATE_UNDONE: &str = "undone";

/// Дискриминанты тира риска (значения `agent_actions.risk_tier`) — зеркало [`super::classify::RiskTier::as_str`].
pub const TIER_AUTO: &str = "auto";
pub const TIER_CONFIRM: &str = "confirm";
pub const TIER_HARDBLOCKED: &str = "hardblocked";

/// Вид изменения файла для [`DiffSummary`] — СТРУКТУРНЫЙ дискриминант (new|edit), НЕ содержимое.
/// Локален для audit (зеркало [`crate::agent::event::FileStatus`], но НЕ зависим от него: журнал
/// подотчётности не должен тащить UI-тип). По построению несёт только перечислимый статус.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Новая заметка (create) — статус-токен `new`.
    New,
    /// Правка существующей заметки (edit/frontmatter) — статус-токен `edit`.
    Edit,
    /// Исполнение exec-команды в песочнице (shell/process/git, Фаза-3 SANDBOX-6c) — статус-токен `exec`.
    /// НЕ vault-дифф: exec не порождает changeset-файл, его «изменение» — побочный эффект процесса. По
    /// exec-пути [`DiffSummary`] в долговечный журнал НЕ пишется (`dispatch_exec_decision` ставит
    /// `diff_summary=None`); вариант существует для корректной классификации + будущего exec-резюме, не для
    /// `+N -M`-диффа.
    Exec,
    /// SELF-LEARNING SL-7: авторство/перезапись SKILL.md агентом — статус-токен `skill_save`. Запись на
    /// диск (в skills_root, НЕ vault), поэтому несёт реальный `+N -M`-дифф (create: del=0; overwrite:
    /// del>0). Отдельный токен (не conflate с vault new/edit): навык — не заметка.
    SkillSave,
}

impl ChangeKind {
    /// Структурный токен статуса (`new`|`edit`|`exec`|`skill_save`) — фиксированный набор, не свободный текст.
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::New => "new",
            ChangeKind::Edit => "edit",
            ChangeKind::Exec => "exec",
            ChangeKind::SkillSave => "skill_save",
        }
    }
}

/// **РЕДАКЦИЯ-ГВАРД содержимого для ДОЛГОВЕЧНОГО журнала (AGENT-6, приватность).** Резюме диффа,
/// которое ПО ПОСТРОЕНИЮ не может нести сырой текст заметки: его поля — ТОЛЬКО числовые счётчики строк
/// (`added`/`deleted`) + перечислимый [`ChangeKind`]. НЕТ ни одного `String`-поля → передать через
/// него тело/значение frontmatter/хунк диффа НЕВОЗМОЖНО (нет канала). Единственный конструктор —
/// [`DiffSummary::new`] (принимает `u32 + u32 + ChangeKind`), единственный рендер — [`DiffSummary::render`]
/// (`"+N -M (new|edit)"`). Любой писатель `agent_actions.diff_summary` ОБЯЗАН строить значение ТОЛЬКО
/// через этот тип — тогда колонка журнала структурно свободна от содержимого пользователя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffSummary {
    /// Число ДОБАВЛЕННЫХ строк (счётчик — не содержимое).
    added: u32,
    /// Число УДАЛЁННЫХ строк (счётчик — не содержимое).
    deleted: u32,
    /// Структурный вид изменения (new|edit) — перечислимый, не текст.
    kind: ChangeKind,
}

impl DiffSummary {
    /// Собрать резюме из СЧЁТЧИКОВ строк (добавлено/удалено) и [`ChangeKind`]. Сигнатура принимает
    /// ТОЛЬКО `u32 + u32 + ChangeKind` — нет параметра-строки, поэтому сырое содержимое заметки
    /// физически не может попасть в журнал через этот путь (структурная редакция).
    pub fn new(added: u32, deleted: u32, kind: ChangeKind) -> Self {
        Self {
            added,
            deleted,
            kind,
        }
    }

    /// Долговечная форма для `agent_actions.diff_summary`: `"+N -M (new|edit)"`. Только счётчики +
    /// статус-токен — никакого содержимого. Это ЕДИНСТВЕННЫЙ способ получить строку для колонки.
    pub fn render(&self) -> String {
        format!("+{} -{} ({})", self.added, self.deleted, self.kind.as_str())
    }
}

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
    /// СТРУКТУРНОЕ, СВОБОДНОЕ ОТ СОДЕРЖИМОГО резюме диффа (`"+N -M (new|edit)"`) — строится ТОЛЬКО через
    /// [`DiffSummary::render`] (AGENT-6). НЕ хранит тело/значения/хунки заметки (редакция-гвард по
    /// построению). `None` — у действий без диффа (например крашнутая строка-якорь до вычисления).
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

/// Переход НЕтерминального состояния БЕЗ фиксации исхода (например `proposed → approved`, AGENT-3d).
///
/// В отличие от [`finish`], outcome НЕ ставится — строка остаётся НЕтерминальной (продолжит жить:
/// apply допишет свою executing→executed-строку, либо последующий finish терминирует). Фенс
/// fail-closed: `WHERE idempotency_key=? AND state=?from AND outcome IS NULL` — переход применяется,
/// ТОЛЬКО если строка действительно в ожидаемом исходном состоянии И ещё не терминирована (нельзя
/// «одобрить» уже отклонённое/исполненное/чужое-состояние действие — гонка/двойное решение отбивается).
/// Возвращает `true`, если ровно эта строка переведена этим вызовом.
pub async fn transition(writer: &WriteActor, key: &str, from: &str, to: &str) -> DbResult<bool> {
    let (key, from, to) = (key.to_string(), from.to_string(), to.to_string());
    writer
        .transaction(move |tx| {
            let n = tx.execute(
                "UPDATE agent_actions SET state=?3, updated_at=?4 \
                 WHERE idempotency_key=?1 AND state=?2 AND outcome IS NULL",
                params![key, from, to, now_secs()],
            )?;
            Ok(n > 0)
        })
        .await
}

/// TTL «зависшего» EXECUTING (SANDBOX-6c-3 §6 crash-recovery): 600с = 5× `DEFAULT_EXEC_TIMEOUT_MS` (120с).
/// Один exec НЕ может легитимно превысить свой 120с-кэп (RealExecRunner kill'ит по таймауту), поэтому за 600с
/// процесс гарантированно мёртв → строку безопасно финализировать FAILED. Owner-tunable (§12.5).
pub const EXEC_STALE_TTL_SECS: i64 = 600;

/// Crash-recovery РИПЕР зависших exec (SANDBOX-6c-3, спека §6 TTL): помечает `FAILED` строки `agent_actions`,
/// застрявшие в `EXECUTING` (`outcome IS NULL`) дольше `older_than_secs` — контейнер исчез (краш/kill/OOM/
/// host-restart) ПОСЛЕ redeem (`APPROVED→EXECUTING`) но ДО `report`, поэтому [`finish`] не сработал, а
/// in-memory `in_flight`-карта (единственное, что могло бы финализировать) потеряна на рестарте host.
///
/// **MARK FAILED, НЕ requeue** (в отличие от [`crate::agent::requeue_stale_running`] для прогонов): exec НЕ
/// replay-safe — одноразовый `exec_token` консьюмнут на redeem, а частичный `rm`/`git`/spawn мог уже
/// произойти; повторный запуск небезопасен. Reaped-строка остаётся БЕЗ undo-хэндла ⇒ корректно вне
/// [`actions_for_undo`] (необратима). Фенс `outcome IS NULL` делает рипер взаимно-исключающим с CAS [`finish`]:
/// кто записал `outcome` первым — победил (поздний `report` после рипера → `finish`=false, FAILED-запись
/// стоит — first-terminal-wins, согласовано с поглощающей семантикой `finish`). `now` ЯВНЫЙ (детерминизм
/// тестов — единый источник с `cutoff`; прод передаёт [`crate::scheduler::now_secs`]). Возвращает число
/// финализированных строк. Live-валидация (kill контейнера до report) — Tier-2 6c-3.
pub async fn reconcile_stale_executing(
    writer: &WriteActor,
    older_than_secs: i64,
    now: i64,
) -> DbResult<usize> {
    writer
        .transaction(move |tx| {
            let cutoff = now - older_than_secs;
            tx.execute(
                "UPDATE agent_actions SET state=?1, outcome=?2, updated_at=?3 \
                 WHERE state=?4 AND outcome IS NULL AND updated_at < ?5",
                params![
                    STATE_FAILED,
                    "exec: контейнер исчез до report (crash-recovery reaper §6 TTL)",
                    now,
                    STATE_EXECUTING,
                    cutoff
                ],
            )
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

/// Только `id` строки по idempotency_key (`None` — нет такой). Лёгкая выборка для гейта автономии:
/// при дубле ключа предложения (то же действие предложено повторно в прогоне) берём существующий
/// `action_id` без полного [`lookup`]. Зеркалит идемпотентность record_before.
pub async fn lookup_id(reader: &ReadPool, key: &str) -> Option<i64> {
    let key = key.to_string();
    reader
        .query(move |c| {
            c.query_row(
                "SELECT id FROM agent_actions WHERE idempotency_key=?1",
                [key],
                |r| r.get::<_, i64>(0),
            )
            .optional()
        })
        .await
        .ok()
        .flatten()
}

/// Действия прогона, ПОДЛЕЖАЩИЕ откату (AGENT-4): строки в state `executed` с НЕ-NULL `undo_kind`,
/// упорядоченные NEWEST-FIRST (`id DESC` = обратный порядок применения). `undo_run` идёт по ним так,
/// чтобы откатить сначала самое позднее действие — зависимые правки разматываются корректно (две правки
/// одной заметки v0→v1→v2 откатываются v2-снапшот(=v1) затем v1-снапшот(=v0) → итог v0).
///
/// Фильтр `state='executed'` — ЕДИНСТВЕННЫЙ источник идемпотентности отбора: уже откаченные строки
/// (`state='undone'`) сюда НЕ попадают, поэтому повторный `undo_run` видит пустой набор (no-op). Failed/
/// proposed/rejected действия НЕ откатываются (диск ими не менялся / отката нет). `undo_kind IS NOT NULL`
/// отсекает теоретические executed-строки без хэндла (их откатить нечем) — fail-closed.
pub async fn actions_for_undo(reader: &ReadPool, run_id: i64) -> DbResult<Vec<ActionRow>> {
    reader
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT id,run_id,idempotency_key,tool_name,target_rel,risk_tier,state,content_hash,\
                 undo_kind,undo_ref,outcome,diff_summary,created_at,updated_at \
                 FROM agent_actions \
                 WHERE run_id=?1 AND state='executed' AND undo_kind IS NOT NULL \
                 ORDER BY id DESC",
            )?;
            let rows = stmt
                .query_map([run_id], row_to_action)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Помечает успешное действие ОТКАЧЕННЫМ (AGENT-4): переход `executed → undone`. ИДЕМПОТЕНТЕН и
/// fail-closed через фенс `WHERE idempotency_key=? AND state='executed'`:
/// - строка в `executed` → переводится в `undone`, возвращает `true` (этот вызов её откатил);
/// - строка уже `undone` (повторный undo / гонка двух откатчиков) → 0 строк обновлено, `false` (no-op);
/// - строка в любом ином state (executing/failed/audited/…) → не трогаем, `false`.
///
/// `outcome` НЕ трогаем — он зафиксирован при executed (подотчётность исхода apply сохраняется); undo
/// меняет ТОЛЬКО `state` + `updated_at`. Это НЕ [`finish`] (тот фенсится на `outcome IS NULL` и здесь
/// не сработал бы — у executed-строки outcome уже есть): отдельный санкционированный переход по state.
pub async fn mark_undone(writer: &WriteActor, key: &str) -> DbResult<bool> {
    let key = key.to_string();
    writer
        .transaction(move |tx| {
            let n = tx.execute(
                "UPDATE agent_actions SET state=?2, updated_at=?3 \
                 WHERE idempotency_key=?1 AND state=?4",
                params![key, STATE_UNDONE, now_secs(), STATE_EXECUTED],
            )?;
            Ok(n > 0)
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

    // ── SANDBOX-6c-3 §6: reconcile_stale_executing (crash-recovery reaper) ────────────────────────
    fn entry_state(run_id: i64, key: &str, state: &str) -> ActionEntry {
        ActionEntry {
            state: state.to_string(),
            ..entry(run_id, key)
        }
    }

    /// Зависший EXECUTING (updated_at < cutoff) → FAILED + структурный outcome + undo None; returns 1.
    #[tokio::test]
    async fn reconcile_marks_stale_executing_failed() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(1, "stale")).await.unwrap();
        let t = lookup(db.reader(), "stale")
            .await
            .unwrap()
            .unwrap()
            .updated_at;
        // now далеко в будущем → cutoff > t → строка зависшая.
        let n = reconcile_stale_executing(db.writer(), 1, t + 1000)
            .await
            .unwrap();
        assert_eq!(n, 1, "одна зависшая строка финализирована");
        let row = lookup(db.reader(), "stale").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_FAILED);
        assert!(
            row.outcome
                .as_deref()
                .unwrap_or("")
                .contains("контейнер исчез"),
            "структурный crash-outcome: {:?}",
            row.outcome
        );
        assert!(
            row.undo_kind.is_none(),
            "reaped exec без undo-хэндла (необратим)"
        );
    }

    /// Свежий EXECUTING (в пределах TTL) НЕ трогается — не риперим живой долгий exec.
    #[tokio::test]
    async fn reconcile_skips_fresh_executing() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(1, "fresh")).await.unwrap();
        let t = lookup(db.reader(), "fresh")
            .await
            .unwrap()
            .unwrap()
            .updated_at;
        // cutoff = (t+1) - 10000 < t → строка НЕ старше TTL.
        let n = reconcile_stale_executing(db.writer(), 10_000, t + 1)
            .await
            .unwrap();
        assert_eq!(n, 0, "свежий EXECUTING не реапится");
        let row = lookup(db.reader(), "fresh").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_EXECUTING, "остаётся executing");
        assert!(row.outcome.is_none());
    }

    /// Только EXECUTING+outcome-NULL: proposed/executed (терминал) НЕ трогаются даже будучи «старыми».
    #[tokio::test]
    async fn reconcile_ignores_non_executing_and_terminal() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(1, "exec")).await.unwrap();
        record_before(db.writer(), entry(1, "done")).await.unwrap();
        finish(db.writer(), "done", STATE_EXECUTED, "ок", None)
            .await
            .unwrap();
        record_before(db.writer(), entry_state(1, "prop", STATE_PROPOSED))
            .await
            .unwrap();
        let t = lookup(db.reader(), "exec")
            .await
            .unwrap()
            .unwrap()
            .updated_at;

        let n = reconcile_stale_executing(db.writer(), 1, t + 1000)
            .await
            .unwrap();
        assert_eq!(n, 1, "реапнут только зависший executing");
        assert_eq!(
            lookup(db.reader(), "exec").await.unwrap().unwrap().state,
            STATE_FAILED
        );
        assert_eq!(
            lookup(db.reader(), "done").await.unwrap().unwrap().state,
            STATE_EXECUTED,
            "терминал (executed) не тронут"
        );
        assert_eq!(
            lookup(db.reader(), "prop").await.unwrap().unwrap().state,
            STATE_PROPOSED,
            "proposed не тронут"
        );
    }

    /// Взаимоисключение с finish (outcome IS NULL фенс): finish первым → рипер no-op; рипер первым → поздний
    /// finish=false (first-terminal-wins).
    #[tokio::test]
    async fn reconcile_idempotent_vs_finish() {
        let (_d, db) = open().await;
        // (a) finish первым → рипер не трогает.
        record_before(db.writer(), entry(1, "a")).await.unwrap();
        finish(db.writer(), "a", STATE_EXECUTED, "репортнут", None)
            .await
            .unwrap();
        let ta = lookup(db.reader(), "a").await.unwrap().unwrap().updated_at;
        assert_eq!(
            reconcile_stale_executing(db.writer(), 1, ta + 1000)
                .await
                .unwrap(),
            0,
            "репортнутую (executed) рипер не трогает"
        );
        assert_eq!(
            lookup(db.reader(), "a").await.unwrap().unwrap().state,
            STATE_EXECUTED
        );
        // (b) рипер первым → поздний finish видит терминал → false, FAILED стоит.
        record_before(db.writer(), entry(1, "b")).await.unwrap();
        let tb = lookup(db.reader(), "b").await.unwrap().unwrap().updated_at;
        assert_eq!(
            reconcile_stale_executing(db.writer(), 1, tb + 1000)
                .await
                .unwrap(),
            1
        );
        let late = finish(db.writer(), "b", STATE_EXECUTED, "поздний report", None)
            .await
            .unwrap();
        assert!(
            !late,
            "поздний finish после рипера → false (first-terminal-wins)"
        );
        assert_eq!(
            lookup(db.reader(), "b").await.unwrap().unwrap().state,
            STATE_FAILED,
            "FAILED от рипера стоит"
        );
    }

    /// Reaped FAILED-строка без undo_kind ⇒ actions_for_undo её НЕ возвращает (необратима).
    #[tokio::test]
    async fn reconciled_row_not_undoable() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(5, "u")).await.unwrap();
        let t = lookup(db.reader(), "u").await.unwrap().unwrap().updated_at;
        reconcile_stale_executing(db.writer(), 1, t + 1000)
            .await
            .unwrap();
        let undoable = actions_for_undo(db.reader(), 5).await.unwrap();
        assert!(
            undoable.is_empty(),
            "reaped exec не в наборе отката (state=failed, undo None)"
        );
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

    /// transition: proposed→approved меняет state, НЕ ставит outcome (строка остаётся НЕтерминальной).
    #[tokio::test]
    async fn transition_changes_state_keeps_outcome_null() {
        let (_d, db) = open().await;
        let mut e = entry(1, "t");
        e.state = STATE_PROPOSED.to_string();
        record_before(db.writer(), e).await.unwrap();

        assert!(
            transition(db.writer(), "t", STATE_PROPOSED, STATE_APPROVED)
                .await
                .unwrap(),
            "proposed→approved применён"
        );
        let row = lookup(db.reader(), "t").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_APPROVED);
        assert!(row.outcome.is_none(), "transition НЕ ставит outcome");
        assert!(!row.is_terminal(), "одобренная строка ещё не терминальна");
    }

    /// transition fail-closed: переход НЕ применяется, если строка не в ожидаемом from-состоянии
    /// (двойное решение/гонка) ИЛИ уже терминирована (outcome зафиксирован).
    #[tokio::test]
    async fn transition_fail_closed_on_wrong_state_or_terminal() {
        let (_d, db) = open().await;
        let mut e = entry(1, "g");
        e.state = STATE_PROPOSED.to_string();
        record_before(db.writer(), e).await.unwrap();

        // from не совпадает (ожидаем approved, а строка proposed) → no-op.
        assert!(
            !transition(db.writer(), "g", STATE_APPROVED, STATE_EXECUTING)
                .await
                .unwrap(),
            "несовпадение from → переход не применён"
        );
        let row = lookup(db.reader(), "g").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_PROPOSED, "state не тронут");

        // Терминируем (reject с исходом) — после этого transition больше не применим.
        finish(db.writer(), "g", STATE_REJECTED, "отклонено", None)
            .await
            .unwrap();
        assert!(
            !transition(db.writer(), "g", STATE_REJECTED, STATE_APPROVED)
                .await
                .unwrap(),
            "терминированную (outcome задан) строку нельзя «переодобрить»"
        );
        let row = lookup(db.reader(), "g").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_REJECTED, "исход reject сохранён");
        assert_eq!(row.outcome.as_deref(), Some("отклонено"));
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

    /// Готовит executed-строку с undo-хэндлом (как оставил бы apply): record_before + finish(executed,
    /// undo). Возвращает её ключ — для последующих actions_for_undo / mark_undone.
    async fn executed_with_undo(db: &Database, run_id: i64, key: &str, undo: UndoCols) -> String {
        record_before(db.writer(), entry(run_id, key))
            .await
            .unwrap();
        finish(db.writer(), key, STATE_EXECUTED, "ok", Some(undo))
            .await
            .unwrap();
        key.to_string()
    }

    /// actions_for_undo: только executed-строки прогона с undo_kind, NEWEST-FIRST (id DESC). failed /
    /// другой run_id / executed-без-undo НЕ попадают.
    #[tokio::test]
    async fn actions_for_undo_filters_and_orders() {
        let (_d, db) = open().await;
        let snap = |ts: &str| UndoCols {
            kind: "snapshot".to_string(),
            reference: ts.to_string(),
        };
        // Прогон 1: два откатываемых действия (a — раньше, b — позже).
        executed_with_undo(&db, 1, "a", snap("100")).await;
        executed_with_undo(&db, 1, "b", snap("200")).await;
        // Прогон 1: failed (диск не менялся) — НЕ откатываем.
        record_before(db.writer(), entry(1, "failed"))
            .await
            .unwrap();
        finish(db.writer(), "failed", STATE_FAILED, "упало", None)
            .await
            .unwrap();
        // Прогон 1: executed БЕЗ undo_kind — откатить нечем, отсекаем.
        record_before(db.writer(), entry(1, "noundo"))
            .await
            .unwrap();
        finish(db.writer(), "noundo", STATE_EXECUTED, "ok", None)
            .await
            .unwrap();
        // Прогон 2: чужой — не должен попасть в выборку прогона 1.
        executed_with_undo(&db, 2, "other", snap("999")).await;

        let rows = actions_for_undo(db.reader(), 1).await.unwrap();
        let keys: Vec<&str> = rows.iter().map(|r| r.idempotency_key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["b", "a"],
            "только executed+undo прогона 1, NEWEST-FIRST (b позже a)"
        );
    }

    /// mark_undone: executed → undone идемпотентно. Первый вызов true (откатил), второй false (no-op);
    /// outcome НЕ тронут (подотчётность исхода apply сохранена). actions_for_undo больше не вернёт строку.
    #[tokio::test]
    async fn mark_undone_idempotent_keeps_outcome() {
        let (_d, db) = open().await;
        let undo = UndoCols {
            kind: "trash".to_string(),
            reference: "Notes/N.md".to_string(),
        };
        executed_with_undo(&db, 1, "k", undo).await;

        assert!(
            mark_undone(db.writer(), "k").await.unwrap(),
            "executed → undone: первый вызов откатил"
        );
        let row = lookup(db.reader(), "k").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_UNDONE);
        assert_eq!(row.outcome.as_deref(), Some("ok"), "outcome НЕ тронут undo");

        // Второй вызов — no-op (строка уже undone): идемпотентность на уровне ledger.
        assert!(
            !mark_undone(db.writer(), "k").await.unwrap(),
            "повторный mark_undone — no-op"
        );
        // И из набора к откату строка ушла.
        assert!(
            actions_for_undo(db.reader(), 1).await.unwrap().is_empty(),
            "undone-строка больше не подлежит откату"
        );
    }

    /// mark_undone fail-closed: НЕ executed-строку (failed) откатить нельзя — no-op, state не тронут.
    #[tokio::test]
    async fn mark_undone_fail_closed_on_non_executed() {
        let (_d, db) = open().await;
        record_before(db.writer(), entry(1, "f")).await.unwrap();
        finish(db.writer(), "f", STATE_FAILED, "упало", None)
            .await
            .unwrap();
        assert!(
            !mark_undone(db.writer(), "f").await.unwrap(),
            "failed нельзя пометить undone"
        );
        let row = lookup(db.reader(), "f").await.unwrap().unwrap();
        assert_eq!(row.state, STATE_FAILED, "state failed не тронут");
    }

    /// REDACTION-GUARD (AGENT-6): DiffSummary рендерит ТОЛЬКО счётчики + статус-токен — `"+N -M (kind)"`.
    /// Структурная гарантия: тип не имеет String-поля, поэтому сырое содержимое в него не попадает.
    #[test]
    fn diff_summary_renders_counts_and_status_only() {
        assert_eq!(
            DiffSummary::new(3, 1, ChangeKind::Edit).render(),
            "+3 -1 (edit)"
        );
        assert_eq!(
            DiffSummary::new(5, 0, ChangeKind::New).render(),
            "+5 -0 (new)"
        );
        assert_eq!(
            DiffSummary::new(0, 0, ChangeKind::Edit).render(),
            "+0 -0 (edit)"
        );
        assert_eq!(ChangeKind::New.as_str(), "new");
        assert_eq!(ChangeKind::Edit.as_str(), "edit");
    }

    /// REDACTION-GUARD (AGENT-6): рендер диффа НИКОГДА не несёт «содержимое» — даже если бы счётчики были
    /// получены из заметки с секретом, выход состоит ИСКЛЮЧИТЕЛЬНО из ASCII-цифр, знаков `+-()` и
    /// фиксированных токенов `new`/`edit`. Доказываем форматом: рендер матчит строгий шаблон.
    #[test]
    fn diff_summary_render_is_structural_only() {
        for (a, d, k) in [
            (0u32, 0u32, ChangeKind::New),
            (42, 7, ChangeKind::Edit),
            (1, 1000, ChangeKind::New),
        ] {
            let s = DiffSummary::new(a, d, k).render();
            // Только цифры, пробелы, +-() и буквы из {new,edit} — никакого произвольного текста.
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_digit() || " +-()newdit".contains(c)),
                "рендер содержит только структурные символы: {s:?}"
            );
            assert_eq!(s, format!("+{a} -{d} ({})", k.as_str()));
        }
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
