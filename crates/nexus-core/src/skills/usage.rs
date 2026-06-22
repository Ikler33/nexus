//! Телеметрия и lifecycle скиллов агента (SELF-LEARNING SL-1) — data-слой над `agent_skill_usage`
//! (миграция 023). Порт hermes `skill_usage.py` + ядра `curator.py` на наш SQLite-идиом: вместо
//! JSON-sidecar'а с fcntl-блокировкой — единственный [`WriteActor`] (атомарность/сериализация
//! «бесплатно», ADR-003), чтение из [`ReadPool`].
//!
//! ## Что здесь (SL-1) и чего НЕТ
//! Это ЧИСТЫЙ data-слой: read/bump/lifecycle-примитивы + их инварианты. НЕТ проводки в tool-loop
//! (bump на `activate_skill`/`read_skill_resource` — SL-2), НЕТ `skill_save`-tool (SL-7), НЕТ
//! scheduler-джобы curator'а (SL-curator), НЕТ UI-панели (SL-ui). Тут — фундамент, на который они
//! сядут.
//!
//! ## Развязка происхождения (keystone, порт hermes дословно)
//! - **Телеметрия** ([`bump_use`]/[`bump_view`]/[`bump_save`]/[`bump_patch`]) пишется для ЛЮБОГО
//!   скилла, независимо от происхождения — чистая наблюдаемость (сколько раз активирован/просмотрен).
//!   Первое касание АПСЕРТит строку (`created_at=now`, `created_by=NULL`).
//! - **Lifecycle** ([`set_state`]/[`set_pinned`]) — NO-OP, если строка не помечена `created_by='agent'`
//!   ([`mark_agent_created`]). vendor/user-скиллы (`created_by` IS NULL / 'vendor' / 'user') неизменяемы
//!   для curator'а: их нельзя архивировать/пинить через этот слой. Enforce — `WHERE created_by='agent'`
//!   в самом UPDATE (а не только проверкой в коде): даже прямой вызов мутатора по vendor-скиллу меняет
//!   0 строк и возвращает `false`. Это и есть «curator НИКОГДА не трогает не-свои скиллы», fail-closed.
//!
//! `forget_orphans` — исключение: это GC ОРФАН-строк телеметрии (скиллы удалены с диска). Он
//! СТРУКТУРНО orphan-only — удаляет лишь строки, чьё имя НЕ в переданном «живом» наборе, поэтому
//! живой скилл (vendor или agent с lifecycle-состоянием) снести нельзя. Curator НИКОГДА не удаляет
//! ЖИВОЙ скилл — только архивирует (обратимо) и только свой.

use rusqlite::params;

use crate::db::{DbResult, ReadPool, WriteActor};
use crate::scheduler::now_secs;

/// Происхождение, помечающее строку как подконтрольную curator'у. Только строки с этим `created_by`
/// поддаются lifecycle-мутациям ([`set_state`]/[`set_pinned`]); остальные неизменяемы (см. доку модуля).
pub const CREATED_BY_AGENT: &str = "agent";

/// Lifecycle-состояние agent-скилла (зеркало hermes active/stale/archived). Закрытый набор —
/// совпадает с `CHECK(state IN (...))` миграции 023, поэтому невалидное значение в БД невозможно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillState {
    /// Используется/свежий — в обычной выдаче.
    Active,
    /// Давно не используется — кандидат на архивацию (curator подсвечивает, не трогает).
    Stale,
    /// Заархивирован curator'ом/пользователем (обратимо) — вне обычной выдачи.
    Archived,
}

impl SkillState {
    /// Строковое представление для колонки `state` (совпадает с CHECK-ограничением).
    pub fn as_str(self) -> &'static str {
        match self {
            SkillState::Active => "active",
            SkillState::Stale => "stale",
            SkillState::Archived => "archived",
        }
    }

    /// Разбор из колонки `state`. Неизвестное → `None` (не паникуем на «новом» состоянии из БД новее).
    /// Не `FromStr` (не нужен generic-parse-контракт): простой db-маппинг закрытого набора.
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "active" => Some(SkillState::Active),
            "stale" => Some(SkillState::Stale),
            "archived" => Some(SkillState::Archived),
            _ => None,
        }
    }
}

/// Какой счётчик/таймштамп инкрементить (закрытый набор → SQL-литералы статичны, без динамической
/// сборки строк из ввода). Зеркало hermes-событий use/view/save/patch.
#[derive(Debug, Clone, Copy)]
enum UsageKind {
    /// `activate_skill` — активирована инструкция (hermes `record_use`).
    Use,
    /// `read_skill_resource` — прочитан ресурс скилла (hermes `record_view`).
    View,
    /// `skill_save` — скилл создан/перезаписан агентом (SL-7).
    Save,
    /// `skill_patch`/консолидация curator'ом (будущее).
    Patch,
}

impl UsageKind {
    /// Полный статичный upsert: первое касание вставляет строку (`created_at=now`, `created_by=NULL`,
    /// `state='active'` по дефолту колонки), повторное — инкрементит счётчик и обновляет `last_*_at`.
    /// `created_at` ставится ТОЛЬКО при вставке (не трогается в DO UPDATE) — это «возраст» строки.
    fn upsert_sql(self) -> &'static str {
        match self {
            UsageKind::Use => {
                "INSERT INTO agent_skill_usage(skill_name, use_count, last_used_at, created_at) \
                 VALUES(?1, 1, ?2, ?2) \
                 ON CONFLICT(skill_name) DO UPDATE SET use_count = use_count + 1, last_used_at = ?2"
            }
            UsageKind::View => {
                "INSERT INTO agent_skill_usage(skill_name, view_count, last_viewed_at, created_at) \
                 VALUES(?1, 1, ?2, ?2) \
                 ON CONFLICT(skill_name) DO UPDATE SET view_count = view_count + 1, last_viewed_at = ?2"
            }
            UsageKind::Save => {
                "INSERT INTO agent_skill_usage(skill_name, save_count, last_saved_at, created_at) \
                 VALUES(?1, 1, ?2, ?2) \
                 ON CONFLICT(skill_name) DO UPDATE SET save_count = save_count + 1, last_saved_at = ?2"
            }
            UsageKind::Patch => {
                "INSERT INTO agent_skill_usage(skill_name, patch_count, last_patched_at, created_at) \
                 VALUES(?1, 1, ?2, ?2) \
                 ON CONFLICT(skill_name) DO UPDATE SET patch_count = patch_count + 1, last_patched_at = ?2"
            }
        }
    }
}

/// Одна строка `agent_skill_usage` — телеметрия + lifecycle одного скилла (PK = `Skill::name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    pub skill_name: String,
    pub use_count: i64,
    pub view_count: i64,
    pub save_count: i64,
    pub patch_count: i64,
    pub last_used_at: Option<i64>,
    pub last_viewed_at: Option<i64>,
    pub last_saved_at: Option<i64>,
    pub last_patched_at: Option<i64>,
    pub created_at: i64,
    /// Происхождение: `Some("agent")` → подконтролен curator'у; NULL/'vendor'/'user' → неизменяем.
    pub created_by: Option<String>,
    /// Lifecycle-состояние (`None`, если в БД новее появилось неизвестное значение — не падаем).
    pub state: Option<SkillState>,
    pub pinned: bool,
    pub archived_at: Option<i64>,
}

impl UsageRecord {
    /// Подконтролен ли curator'у (lifecycle-мутации разрешены): `created_by == 'agent'`.
    pub fn is_agent_created(&self) -> bool {
        self.created_by.as_deref() == Some(CREATED_BY_AGENT)
    }

    /// Якорь «последней активности» = max(last_used/viewed/saved/patched); если касаний не было —
    /// fallback на `created_at` (никогда-не-юзанный agent-скилл всё равно прунабелен по возрасту,
    /// hermes `_idle_days`). Используется curator'ом для LRU/age-пруна и UI-сортировки.
    pub fn last_activity(&self) -> i64 {
        [
            self.last_used_at,
            self.last_viewed_at,
            self.last_saved_at,
            self.last_patched_at,
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(self.created_at)
    }
}

/// Колонки SELECT в порядке, ожидаемом [`row_to_record`].
const SELECT_COLS: &str = "skill_name, use_count, view_count, save_count, patch_count, \
     last_used_at, last_viewed_at, last_saved_at, last_patched_at, created_at, created_by, \
     state, pinned, archived_at";

fn row_to_record(r: &rusqlite::Row) -> rusqlite::Result<UsageRecord> {
    let state_raw: Option<String> = r.get(11)?;
    Ok(UsageRecord {
        skill_name: r.get(0)?,
        use_count: r.get(1)?,
        view_count: r.get(2)?,
        save_count: r.get(3)?,
        patch_count: r.get(4)?,
        last_used_at: r.get(5)?,
        last_viewed_at: r.get(6)?,
        last_saved_at: r.get(7)?,
        last_patched_at: r.get(8)?,
        created_at: r.get(9)?,
        created_by: r.get(10)?,
        state: state_raw.as_deref().and_then(SkillState::from_db),
        pinned: r.get::<_, i64>(12)? != 0,
        archived_at: r.get(13)?,
    })
}

/// SQL-зеркало [`UsageRecord::last_activity`]: greatest-of четырёх `last_*_at` с fallback на
/// `created_at`. SQLite-`max(...)` со скаляр-аргументами — это «наибольший», НО возвращает NULL, если
/// ЛЮБОЙ аргумент NULL → каждый nullable обёрнут `COALESCE(...,0)`; `created_at` NOT NULL якорит
/// нетронутую строку (как `unwrap_or(created_at)` в Rust). Держать ОДНО определение в двух местах
/// (Rust + SQL) — иначе ORDER BY curator'а разойдётся с тем, что считает «активностью» остальной код
/// (ревью SL-1, MAJOR: `COALESCE`=первый-не-NULL ≠ max → свежепатченный скилл всплывал в топ
/// архивного списка). Реальные таймштампы > 0, поэтому 0-floor не конфликтует с настоящей активностью.
const LAST_ACTIVITY_SQL: &str = "max(COALESCE(last_used_at, 0), COALESCE(last_viewed_at, 0), \
     COALESCE(last_saved_at, 0), COALESCE(last_patched_at, 0), created_at)";

/// Валиден ли `skill_name` как ключ строки (defense-in-depth). В норме сюда приходит уже
/// провалидированный [`crate::skills::Skill::name`] (parse_skill отвергает пустое/огромное/control/
/// разделители при загрузке), но data-слой НЕ полагается на это: невалидное имя → операция-no-op
/// (не плодим мусорную PK-строку, не раздуваем таблицу). Зеркалит ключевые проверки `validate_name`:
/// непустой, ≤128 байт, без control-символов. Не тримим (телеметрия и lifecycle обязаны ключевать
/// ОДНУ строку — нормализация рассинхронизировала бы ключ).
fn valid_skill_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.chars().any(|c| c.is_control())
}

// ── Телеметрия (для ЛЮБОГО скилла, апсерт) ───────────────────────────────────────────────────────

/// Инкремент `use_count` + `last_used_at=now` (создаёт строку при первом касании). `activate_skill`.
pub async fn bump_use(writer: &WriteActor, skill_name: &str) -> DbResult<()> {
    bump(writer, UsageKind::Use, skill_name).await
}

/// Инкремент `view_count` + `last_viewed_at=now`. `read_skill_resource`.
pub async fn bump_view(writer: &WriteActor, skill_name: &str) -> DbResult<()> {
    bump(writer, UsageKind::View, skill_name).await
}

/// Инкремент `save_count` + `last_saved_at=now`. Создание/перезапись скилла агентом (SL-7).
pub async fn bump_save(writer: &WriteActor, skill_name: &str) -> DbResult<()> {
    bump(writer, UsageKind::Save, skill_name).await
}

/// Инкремент `patch_count` + `last_patched_at=now`. Точечная правка/консолидация (будущее).
pub async fn bump_patch(writer: &WriteActor, skill_name: &str) -> DbResult<()> {
    bump(writer, UsageKind::Patch, skill_name).await
}

async fn bump(writer: &WriteActor, kind: UsageKind, skill_name: &str) -> DbResult<()> {
    if !valid_skill_name(skill_name) {
        return Ok(()); // невалидное имя → no-op (не создаём мусорную строку)
    }
    let name = skill_name.to_string();
    let now = now_secs();
    let sql = kind.upsert_sql();
    writer
        .call(move |conn| conn.execute(sql, params![name, now]).map(|_| ()))
        .await
}

// ── Провенанс ────────────────────────────────────────────────────────────────────────────────────

/// Пометить НОВЫЙ скилл как СОЗДАННЫЙ агентом (`created_by='agent'`) — единственное, что открывает
/// скилл для lifecycle-мутаций curator'а. **INSERT-only стамп**: ставит провенанс ТОЛЬКО при создании
/// строки (`ON CONFLICT DO NOTHING`); если строка УЖЕ существует — НЕ трогает её совсем.
///
/// # Почему не «promote NULL→agent» (ревью SL-1, keystone-bypass)
/// Телеметрия ([`bump_use`]/[`bump_view`]…) создаёт строку с `created_by=NULL` при первом касании —
/// в т.ч. когда vendor-скилл легитимно активируют (телеметрия пишется для ВСЕХ). Прежний
/// `DO UPDATE … WHERE created_by IS NULL` промоутил бы такую vendor-строку в 'agent', сделав
/// vendor-скилл управляемым curator'ом (архивируемым) — пробой keystone-инварианта «curator не трогает
/// чужое». `DO NOTHING` это закрывает СТРУКТУРНО: провенанс 'agent' может появиться ТОЛЬКО на свежей
/// строке. Цена — fail-closed-направление: если для agent-скилла телеметрия КАК-ТО прошла раньше
/// стампа, скилл останется неуправляемым (а не «vendor станет управляемым»). Поэтому **agent-origin
/// `skill_save` (SL-7) ОБЯЗАН звать `mark_agent_created` ПЕРВЫМ — до любой телеметрии для этого имени**
/// (естественный порядок: сначала сохраняем скилл, активируют его позже).
pub async fn mark_agent_created(writer: &WriteActor, skill_name: &str) -> DbResult<()> {
    if !valid_skill_name(skill_name) {
        return Ok(()); // невалидное имя → no-op
    }
    let name = skill_name.to_string();
    let now = now_secs();
    writer
        .call(move |conn| {
            conn.execute(
                "INSERT INTO agent_skill_usage(skill_name, created_at, created_by) \
                 VALUES(?1, ?2, 'agent') \
                 ON CONFLICT(skill_name) DO NOTHING",
                params![name, now],
            )
            .map(|_| ())
        })
        .await
}

// ── Lifecycle (ТОЛЬКО для created_by='agent', иначе no-op → false) ────────────────────────────────

/// Сменить lifecycle-состояние agent-скилла. Возвращает `true`, если строка изменена; `false` —
/// строки нет ИЛИ она не `created_by='agent'` (no-op: vendor/user-скилл неизменяем). При переводе в
/// `Archived` ставит `archived_at=now`; в любое другое — `archived_at=NULL` (архивация обратима).
pub async fn set_state(writer: &WriteActor, skill_name: &str, state: SkillState) -> DbResult<bool> {
    if !valid_skill_name(skill_name) {
        return Ok(false); // невалидное имя → no-op
    }
    let name = skill_name.to_string();
    let s = state.as_str();
    let archived_at = if state == SkillState::Archived {
        Some(now_secs())
    } else {
        None
    };
    writer
        .call(move |conn| {
            let n = conn.execute(
                "UPDATE agent_skill_usage SET state = ?2, archived_at = ?3 \
                 WHERE skill_name = ?1 AND created_by = 'agent'",
                params![name, s, archived_at],
            )?;
            Ok(n > 0)
        })
        .await
}

/// Заархивировать agent-скилл (сахар над [`set_state`] c [`SkillState::Archived`]). Обратимо
/// (`set_state(.., Active)` снимает архив и обнуляет `archived_at`). Curator НИКОГДА не удаляет —
/// только архивирует. `false` — не-agent-строка / нет строки (no-op).
pub async fn archive(writer: &WriteActor, skill_name: &str) -> DbResult<bool> {
    set_state(writer, skill_name, SkillState::Archived).await
}

/// Пин/анпин agent-скилла (закреплённый curator не архивирует, сортируется выше). `false` —
/// не-agent-строка / нет строки (no-op).
pub async fn set_pinned(writer: &WriteActor, skill_name: &str, pinned: bool) -> DbResult<bool> {
    if !valid_skill_name(skill_name) {
        return Ok(false); // невалидное имя → no-op
    }
    let name = skill_name.to_string();
    writer
        .call(move |conn| {
            let n = conn.execute(
                "UPDATE agent_skill_usage SET pinned = ?2 \
                 WHERE skill_name = ?1 AND created_by = 'agent'",
                params![name, pinned as i64],
            )?;
            Ok(n > 0)
        })
        .await
}

/// GC ОРФАН-строк телеметрии (скиллы удалены с диска) — hermes `forget`, но **структурно orphan-only**.
/// Удаляет строки `agent_skill_usage`, чьё `skill_name` НЕ входит в `live_skill_names` (актуальный
/// набор имён из `discover_skills`). Файлы скиллов НЕ трогает (этот слой ими не владеет). Возвращает
/// число удалённых строк.
///
/// # Почему батч-по-живым, а не delete-по-имени (ревью SL-1)
/// Прежний `forget(name)` безусловно удалял ЛЮБУЮ строку по имени — мог снести ЖИВОЙ agent-скилл со
/// всем lifecycle-состоянием (pinned/archived/счётчики). Data-слой не может сам проверить «жив ли скилл»
/// (это вопрос ФС). Поэтому контракт инвертирован: вызывающий (curator) передаёт МНОЖЕСТВО живых имён, и
/// строка живого скилла удалена быть НЕ может СТРУКТУРНО — `NOT IN (live)`. Пустой `live` ⇒ живых нет ⇒
/// все строки орфанны (полная очистка). Лимит SQLite на число bind-параметров (≥999) с запасом
/// покрывает реалистичные десятки-сотни скиллов.
pub async fn forget_orphans(writer: &WriteActor, live_skill_names: &[String]) -> DbResult<usize> {
    let live: Vec<String> = live_skill_names.to_vec();
    writer
        .call(move |conn| {
            if live.is_empty() {
                // Живых скиллов нет → каждая строка телеметрии орфанна.
                return conn.execute("DELETE FROM agent_skill_usage", []);
            }
            let placeholders = vec!["?"; live.len()].join(",");
            let sql =
                format!("DELETE FROM agent_skill_usage WHERE skill_name NOT IN ({placeholders})");
            conn.execute(&sql, rusqlite::params_from_iter(live.iter()))
        })
        .await
}

// ── Чтение ───────────────────────────────────────────────────────────────────────────────────────

/// Строка телеметрии конкретного скилла (`None`, если касаний ещё не было).
pub async fn get_record(reader: &ReadPool, skill_name: &str) -> DbResult<Option<UsageRecord>> {
    let name = skill_name.to_string();
    reader
        .query(move |c| {
            let sql = format!("SELECT {SELECT_COLS} FROM agent_skill_usage WHERE skill_name = ?1");
            match c.query_row(&sql, [name], row_to_record) {
                Ok(rec) => Ok(Some(rec)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
}

/// Оверлей ранжирования для меню скиллов (SL-2 будет подмешивать порядок): закреплённые сверху,
/// затем по свежести использования (строки без `last_used_at` — в конец), затем по числу
/// использований, затем по имени (детерминизм). Все строки (любого происхождения).
pub async fn ranked_overlay(reader: &ReadPool) -> DbResult<Vec<UsageRecord>> {
    reader
        .query(move |c| {
            // `(last_used_at IS NULL)` → 0 для использованных (раньше), 1 для нетронутых (позже):
            // явный NULLS-LAST поверх `DESC` (SQLite и так кладёт NULL в конец при DESC, но делаем
            // намерение явным и устойчивым).
            let sql = format!(
                "SELECT {SELECT_COLS} FROM agent_skill_usage \
                 ORDER BY pinned DESC, (last_used_at IS NULL), last_used_at DESC, \
                 use_count DESC, skill_name ASC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map([], row_to_record)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
}

/// Кандидаты curator'а: ТОЛЬКО agent-созданные строки (`created_by='agent'`) — то, чем curator/UI
/// вправе управлять. Порядок — по ПОСЛЕДНЕЙ АКТИВНОСТИ (давние сверху: первые на ревью/архивацию),
/// затем по имени. «Активность» = [`LAST_ACTIVITY_SQL`] (greatest-of last_*, fallback created_at),
/// БАЙТ-в-байт совпадает с [`UsageRecord::last_activity`] — curator/SL-7 могут доверять SQL-порядку для
/// batch-cap прунинга, не пересчитывая в Rust. vendor/user-скиллы сюда не попадают (вне зоны curator'а).
pub async fn agent_created_report(reader: &ReadPool) -> DbResult<Vec<UsageRecord>> {
    reader
        .query(move |c| {
            let sql = format!(
                "SELECT {SELECT_COLS} FROM agent_skill_usage \
                 WHERE created_by = 'agent' \
                 ORDER BY {LAST_ACTIVITY_SQL} ASC, skill_name ASC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map([], row_to_record)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::TempDir;

    async fn temp_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .expect("open db");
        (db, dir)
    }

    /// Сырой seed строки с явными значениями (детерминированные таймштампы для тестов сортировки/якоря).
    #[allow(clippy::too_many_arguments)]
    async fn seed(
        writer: &WriteActor,
        name: &str,
        created_by: Option<&str>,
        pinned: bool,
        use_count: i64,
        last_used_at: Option<i64>,
        created_at: i64,
    ) {
        let name = name.to_string();
        let created_by = created_by.map(|s| s.to_string());
        writer
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_skill_usage(skill_name, use_count, last_used_at, created_at, created_by, pinned) \
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![name, use_count, last_used_at, created_at, created_by, pinned as i64],
                )
                .map(|_| ())
            })
            .await
            .unwrap();
    }

    /// Миграция 023 применена: таблица существует и принимает upsert (косвенно — bump не падает).
    #[tokio::test]
    async fn migration_023_table_present() {
        let (db, _d) = temp_db().await;
        let present: i64 = db
            .reader()
            .query(|c| {
                c.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='agent_skill_usage'",
                    [],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(
            present, 1,
            "таблица agent_skill_usage создана миграцией 023"
        );
    }

    /// Телеметрия: bump инкрементит свой счётчик; created_at стабилен после первой вставки; разные
    /// kind'ы независимы; last_*_at проставляется.
    #[tokio::test]
    async fn bump_increments_and_created_at_is_stable() {
        let (db, _d) = temp_db().await;
        let w = db.writer();

        bump_use(w, "alpha").await.unwrap();
        let r1 = get_record(db.reader(), "alpha").await.unwrap().unwrap();
        assert_eq!(r1.use_count, 1);
        assert!(r1.last_used_at.is_some());
        let created = r1.created_at;

        bump_use(w, "alpha").await.unwrap();
        bump_view(w, "alpha").await.unwrap();
        bump_save(w, "alpha").await.unwrap();
        bump_patch(w, "alpha").await.unwrap();
        let r2 = get_record(db.reader(), "alpha").await.unwrap().unwrap();
        assert_eq!(r2.use_count, 2, "use инкрементнулся дважды");
        assert_eq!(r2.view_count, 1);
        assert_eq!(r2.save_count, 1);
        assert_eq!(r2.patch_count, 1);
        assert_eq!(
            r2.created_at, created,
            "created_at не меняется после вставки"
        );
        assert!(r2.last_viewed_at.is_some() && r2.last_saved_at.is_some());
        assert_eq!(r2.created_by, None, "телеметрия не присваивает провенанс");
    }

    /// Lifecycle-мутаторы — NO-OP для не-agent строки и РАБОТАЮТ после mark_agent_created.
    #[tokio::test]
    async fn lifecycle_noop_unless_agent_created() {
        let (db, _d) = temp_db().await;
        let w = db.writer();

        // Телеметрия создала строку с created_by=NULL (vendor/user-скилл).
        bump_use(w, "vend").await.unwrap();
        assert!(
            !set_state(w, "vend", SkillState::Stale).await.unwrap(),
            "не-agent → no-op"
        );
        assert!(
            !set_pinned(w, "vend", true).await.unwrap(),
            "не-agent → no-op"
        );
        assert!(!archive(w, "vend").await.unwrap(), "не-agent → no-op");
        let r = get_record(db.reader(), "vend").await.unwrap().unwrap();
        assert_eq!(r.state, Some(SkillState::Active), "состояние не тронуто");
        assert!(!r.pinned);
        assert!(r.created_by.is_none());

        // Agent-origin: mark_agent_created на СВЕЖЕМ имени (строки ещё нет) → создаёт 'agent'-строку →
        // мутаторы оживают. (Промоут существующей телеметрии-NULL-строки невозможен — см. отдельный тест.)
        mark_agent_created(w, "mine").await.unwrap();
        assert!(set_state(w, "mine", SkillState::Stale).await.unwrap());
        assert!(set_pinned(w, "mine", true).await.unwrap());
        let r2 = get_record(db.reader(), "mine").await.unwrap().unwrap();
        assert_eq!(r2.created_by.as_deref(), Some("agent"));
        assert_eq!(r2.state, Some(SkillState::Stale));
        assert!(r2.pinned);

        // Архив проставляет archived_at; снятие архива (→Active) обнуляет его.
        assert!(archive(w, "mine").await.unwrap());
        let r3 = get_record(db.reader(), "mine").await.unwrap().unwrap();
        assert_eq!(r3.state, Some(SkillState::Archived));
        assert!(r3.archived_at.is_some(), "archived_at проставлен");
        assert!(set_state(w, "mine", SkillState::Active).await.unwrap());
        let r4 = get_record(db.reader(), "mine").await.unwrap().unwrap();
        assert_eq!(r4.archived_at, None, "снятие архива обнуляет archived_at");
    }

    /// KEYSTONE-регрессия (ревью SL-1): телеметрия по vendor-скиллу создаёт NULL-строку; последующий
    /// mark_agent_created НЕ промоутит её в 'agent' (INSERT-only DO NOTHING) → vendor-скилл остаётся
    /// НЕуправляемым curator'ом. Прежний `DO UPDATE WHERE created_by IS NULL` пробивал бы инвариант.
    #[tokio::test]
    async fn mark_agent_created_does_not_promote_telemetry_row() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        // Легитимная телеметрия по VENDOR-скиллу (его активировали) → строка created_by=NULL.
        bump_view(w, "vendorskill").await.unwrap();
        // Попытка пометить его agent-origin — НЕ должна сработать (строка уже есть).
        mark_agent_created(w, "vendorskill").await.unwrap();
        let r = get_record(db.reader(), "vendorskill")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            r.created_by, None,
            "телеметрия-NULL НЕ промоутится в 'agent'"
        );
        // Следовательно lifecycle-мутаторы по нему остаются no-op.
        assert!(
            !set_state(w, "vendorskill", SkillState::Archived)
                .await
                .unwrap(),
            "vendor-скилл остаётся неуправляемым"
        );
    }

    /// mark_agent_created не клобберит уже непустой провенанс (defense-in-depth).
    #[tokio::test]
    async fn mark_agent_created_does_not_clobber_existing_provenance() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        seed(w, "v", Some("vendor"), false, 0, None, 100).await;
        mark_agent_created(w, "v").await.unwrap();
        let r = get_record(db.reader(), "v").await.unwrap().unwrap();
        assert_eq!(
            r.created_by.as_deref(),
            Some("vendor"),
            "vendor-провенанс сохранён"
        );
    }

    /// [4] defense-in-depth: невалидное skill_name (пустое/огромное/control) → no-op (строка не плодится,
    /// lifecycle возвращает false). В норме сюда приходит провалидированный Skill::name, но слой не верит.
    #[tokio::test]
    async fn invalid_skill_name_is_noop() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        bump_use(w, "").await.unwrap();
        bump_use(w, "with\nnewline").await.unwrap();
        let huge = "x".repeat(200);
        bump_use(w, &huge).await.unwrap();
        // Ни одной строки не создано.
        let n: i64 = db
            .reader()
            .query(|c| c.query_row("SELECT count(*) FROM agent_skill_usage", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(n, 0, "невалидные имена не создали строк");
        assert!(!set_state(w, "", SkillState::Stale).await.unwrap());
        assert!(!set_pinned(w, "", true).await.unwrap());
        assert!(get_record(db.reader(), "").await.unwrap().is_none());
    }

    /// Якорь активности = max(last_*); при отсутствии касаний — fallback на created_at.
    #[tokio::test]
    async fn last_activity_max_with_created_at_fallback() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        seed(w, "untouched", None, false, 0, None, 100).await;
        seed(w, "used", None, false, 1, Some(500), 100).await;
        let untouched = get_record(db.reader(), "untouched").await.unwrap().unwrap();
        let used = get_record(db.reader(), "used").await.unwrap().unwrap();
        assert_eq!(untouched.last_activity(), 100, "fallback на created_at");
        assert_eq!(used.last_activity(), 500, "max от last_used_at");

        // Несколько касаний — берётся максимум (через прямой record).
        let mixed = UsageRecord {
            last_used_at: Some(200),
            last_patched_at: Some(900),
            ..used.clone()
        };
        assert_eq!(mixed.last_activity(), 900);
    }

    /// ranked_overlay: pinned сверху, затем свежесть, нетронутые (last_used NULL) — в конец, затем use_count.
    #[tokio::test]
    async fn ranked_overlay_orders_pinned_recency_use() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        seed(w, "pinned-old", Some("agent"), true, 1, Some(100), 10).await;
        seed(w, "recent", None, false, 1, Some(900), 10).await;
        seed(w, "older", None, false, 5, Some(500), 10).await;
        seed(w, "never", None, false, 9, None, 10).await;

        let order: Vec<String> = ranked_overlay(db.reader())
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.skill_name)
            .collect();
        assert_eq!(
            order,
            vec!["pinned-old", "recent", "older", "never"],
            "pinned → свежее → нетронутое в конец"
        );
    }

    /// agent_created_report возвращает ТОЛЬКО agent-строки, давние по активности — первыми.
    #[tokio::test]
    async fn agent_report_only_agent_rows_oldest_first() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        seed(w, "vendorX", None, false, 3, Some(800), 10).await; // не-agent → исключён
        seed(w, "agentRecent", Some("agent"), false, 1, Some(900), 10).await;
        seed(w, "agentStale", Some("agent"), false, 1, Some(200), 10).await;

        let names: Vec<String> = agent_created_report(db.reader())
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.skill_name)
            .collect();
        assert_eq!(
            names,
            vec!["agentStale", "agentRecent"],
            "только agent-строки, давние первыми; vendor исключён"
        );
    }

    /// [1]/[5] MAJOR-регрессия: agent_created_report сортирует по MAX-активности (как last_activity()),
    /// а не по первому-не-NULL. Скилл со СТАРЫМ last_used, но СВЕЖИМ last_patched, должен считаться
    /// активным (внизу архивного списка), а не всплывать наверх по устаревшему last_used.
    #[tokio::test]
    async fn agent_report_orders_by_max_activity_not_first_nonnull() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        // patched-fresh: last_used=100 (старо), last_patched=5000 (свежо) → активность 5000.
        w.call(|c| {
            c.execute(
                "INSERT INTO agent_skill_usage(skill_name, last_used_at, last_patched_at, created_at, created_by) \
                 VALUES('patched-fresh', 100, 5000, 10, 'agent')",
                [],
            )
            .map(|_| ())
        })
        .await
        .unwrap();
        // idle: last_used=200, без patch → активность 200 (реально самый давний).
        seed(w, "idle", Some("agent"), false, 1, Some(200), 10).await;

        let names: Vec<String> = agent_created_report(db.reader())
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.skill_name)
            .collect();
        assert_eq!(
            names,
            vec!["idle", "patched-fresh"],
            "max-семантика: idle (200) — давнее patched-fresh (5000), стоит первым на архивацию"
        );
        // Прежняя COALESCE-семантика дала бы [patched-fresh(100), idle(200)] — баг.
    }

    /// forget_orphans удаляет ТОЛЬКО строки скиллов, отсутствующих в живом наборе; живые (в т.ч.
    /// agent-скилл с lifecycle-состоянием) сохраняются. Пустой live → полная очистка.
    #[tokio::test]
    async fn forget_orphans_removes_only_absent() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        bump_use(w, "alive").await.unwrap();
        bump_use(w, "ghost").await.unwrap();
        seed(w, "alive-agent", Some("agent"), true, 1, Some(500), 10).await;

        // Живые: alive + alive-agent. ghost отсутствует → орфан.
        let removed = forget_orphans(w, &["alive".into(), "alive-agent".into()])
            .await
            .unwrap();
        assert_eq!(removed, 1, "удалён только ghost");
        assert!(get_record(db.reader(), "ghost").await.unwrap().is_none());
        assert!(
            get_record(db.reader(), "alive").await.unwrap().is_some(),
            "живой скилл сохранён"
        );
        assert!(
            get_record(db.reader(), "alive-agent")
                .await
                .unwrap()
                .is_some(),
            "живой agent-скилл с lifecycle-состоянием НЕ удалён"
        );

        // Пустой live → все строки орфанны → полная очистка.
        let removed_all = forget_orphans(w, &[]).await.unwrap();
        assert_eq!(removed_all, 2, "пустой live → удалены оба оставшихся");
        let n: i64 = db
            .reader()
            .query(|c| c.query_row("SELECT count(*) FROM agent_skill_usage", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
