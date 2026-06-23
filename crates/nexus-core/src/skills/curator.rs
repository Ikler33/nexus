//! SL-curator: фоновая scheduler-джоба ЖИЗНЕННОГО ЦИКЛА навыков, СОЗДАННЫХ агентом
//! (`created_by='agent'`, см. [`super::usage`]). Третья и последняя часть write-path само-обучения
//! (после SL-2 телеметрии и SL-7 `skill_save`): агент создаёт навыки → curator поддерживает их «гигиену»
//! без участия владельца, но СТРОГО консервативно.
//!
//! Порт ядра hermes `curator.py` на наш идиом. Образец проводки — [`crate::episode::EpisodeRollupHandler`]
//! (recurring scheduled-only job, persisted-гейт, ранний NOOP при OFF); дисциплина безопасности —
//! [`crate::memory::consolidate`] («fail-closed: при сомнении НЕ разрушаем»).
//!
//! ## Что делает (и чего НИКОГДА)
//! 1. **Lifecycle** `active → stale → archive` по простою ([`UsageRecord::last_activity`]): навык без
//!    активности ≥[`STALE_AFTER_SECS`] → `stale` (виден, но deprioritized); `stale` без активности
//!    ≥[`ARCHIVE_AFTER_SECS`] → `archived` (вне обычной выдачи). ДВУХШАГОВО (не active→archive напрямую):
//!    `stale` — видимый период-предупреждение перед скрытием. Всё ОБРАТИМО ([`usage::set_state`] обратно
//!    в `Active` снимает архив).
//! 2. **GC орфан-телеметрии**: строки `agent_skill_usage` скиллов, УЖЕ удалённых с диска, чистятся
//!    ([`usage::forget_orphans`], structurally orphan-only).
//!
//! **НИКОГДА**: (а) не удаляет ЖИВОЙ навык — ни файл (не владеет ФС), ни строку (forget_orphans only по
//! отсутствующим на диске); (б) не трогает не-agent скиллы (vendor/user) — `agent_created_report` отдаёт
//! ТОЛЬКО `created_by='agent'`, а мутаторы дополнительно фильтруют `WHERE created_by='agent'` в SQL
//! (double-enforce); (в) не трогает `pinned` (владелец закрепил); (г) ничего не делает при выключенном
//! `ai.skills.learning_enabled` (owner-gated, default OFF — fail-closed).
//!
//! ## «Eval-гейт» здесь (аналог [`crate::memory::consolidate`] DELETE-precision)
//! У consolidate авто-удаление гейтится оффлайн-evalом (DELETE-precision ≥ порога), т.к. там LLM-суждение
//! может ошибиться. Здесь LLM НЕТ ВООБЩЕ: решение [`decide`] — ЧИСТАЯ ДЕТЕРМИНИРОВАННАЯ функция времени
//! простоя. Поэтому «eval-гейт» реализован как **инвариант точности, доказанный тестом**: `decide`
//! НИКОГДА не возвращает `MarkStale`/`Archive` для закреплённого ИЛИ активного-в-пределах-порога навыка
//! (precision = 1.0 ПО ПОСТРОЕНИЮ — ложно-архивировать живой навык структурно нельзя). Это СИЛЬНЕЕ
//! LLM-гейта: переходы обратимы И детерминированы, нет недетерминизма для CI. Плюс GC консервативен:
//! чистим телеметрию ТОЛЬКО когда каталог прочитан ЧИСТО (есть навыки И `errors()` пуст) — пустой набор
//! (пустой каталог НЕ отличим от сбоя чтения) ИЛИ любой непарсящийся-но-присутствующий-на-диске навык
//! ⇒ НЕ чистим (иначе снесли бы lifecycle ЖИВОГО навыка). «При сомнении не разрушаем», философия consolidate.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::db::{ReadPool, WriteActor};
use crate::scheduler::{now_secs, Job, JobHandler};
use crate::skills::discover_skills;
use crate::skills::usage::{
    self, agent_created_report, archive, set_state, SkillState, UsageRecord,
};

/// kind планировщика для фоновой курации навыков.
pub const KIND_SKILL_CURATOR: &str = "skill_curator";

/// Простой (сек) до перевода `active → stale`. Консервативно (≈60 дней): архивация скрывает навык,
/// поэтому спешить незачем — даём долгое окно «вдруг ещё пригодится».
pub const STALE_AFTER_SECS: i64 = 60 * 86_400;

/// Простой (сек) до перевода `stale → archived` (≈180 дней / полгода). Считается ОТ ТОЙ ЖЕ активности
/// (set_state не бьёт last_*), поэтому простой монотонно растёт, и долго-неактивный навык доходит до
/// архива за ≥2 прогона (active→stale, затем stale→archive) — без «перепрыга» предупреждающего периода.
pub const ARCHIVE_AFTER_SECS: i64 = 180 * 86_400;

/// Интервал recurring-прогона (раз в сутки). Курация — фоновая гигиена «на простое»; чаще нет смысла.
pub const CURATOR_INTERVAL_SECS: i64 = 24 * 3_600;

/// Анти-flood: максимум lifecycle-переходов за один прогон. Переходы дешёвы (single-row UPDATE, не LLM),
/// но кап ограничивает «взрыв» одного тика — на первом прогоне над большим backlog навыки переходят
/// постепенно (остаток доберёт следующий суточный тик). `Keep` не считается (обычный прогон ≈ no-op).
pub const MAX_TRANSITIONS_PER_RUN: usize = 50;

/// Решение curator'а по ОДНОМУ навыку. Закрытый набор; НИКОГДА нет варианта «удалить» (удаление живого
/// навыка структурно отсутствует — см. доку модуля).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuratorAction {
    /// Оставить как есть (активность свежая / закреплён / уже терминальное состояние).
    Keep,
    /// `active → stale` (давно не используется; виден, но deprioritized).
    MarkStale,
    /// `stale → archived` (давно неактивен; вне обычной выдачи, обратимо).
    Archive,
}

/// ЧИСТОЕ детерминированное решение по навыку — ядро «eval-гейта» (тестируемо без БД/времени/LLM).
///
/// Инварианты точности (доказаны тестами; precision = 1.0 по построению):
/// - `pinned` → ВСЕГДА `Keep` (закреплённый владельцем неприкосновенен);
/// - активность свежее порога → `Keep` (живой навык НИКОГДА ложно не архивируется);
/// - `Archived` (терминально, обратимо лишь вручную) и `None` (неизвестное состояние из БД новее) →
///   `Keep` (fail-closed: не трогаем то, что не понимаем).
///
/// `stale_after`/`archive_after` — параметры (а не глобальные конст.) для дешёвых тестов малыми числами;
/// прод-вызов подставляет [`STALE_AFTER_SECS`]/[`ARCHIVE_AFTER_SECS`].
pub fn decide(
    record: &UsageRecord,
    now: i64,
    stale_after: i64,
    archive_after: i64,
) -> CuratorAction {
    if record.pinned {
        return CuratorAction::Keep;
    }
    let idle = now.saturating_sub(record.last_activity());
    match record.state {
        Some(SkillState::Active) => {
            if idle >= stale_after {
                CuratorAction::MarkStale
            } else {
                CuratorAction::Keep
            }
        }
        Some(SkillState::Stale) => {
            if idle >= archive_after {
                CuratorAction::Archive
            } else {
                CuratorAction::Keep
            }
        }
        // Archived (терминально) | None (неизвестное состояние из новее-БД) → fail-closed Keep.
        _ => CuratorAction::Keep,
    }
}

/// Фоновый хендлер курации (scheduler kind [`KIND_SKILL_CURATOR`]). Держит долю БД (reader/writer),
/// канонический корень skills (для GC живого набора) и флаг `ai.skills.learning_enabled`.
///
/// Гейт-by-construction: `learning_enabled=false` или `skills_root=None` → прогон NOOP (см. [`Self::sweep`]).
/// Композиционный корень (agentd) регистрирует хендлер ТОЛЬКО при `learning_enabled && skills present`,
/// но `sweep` ещё раз защищается (defense-in-depth: даже вручную поставленная джоба при OFF — no-op).
pub struct SkillCuratorHandler {
    reader: ReadPool,
    writer: WriteActor,
    /// КАНОНИЧЕСКИЙ корень skills-каталога (как у [`crate::agent::SkillContext::skills_root`]).
    /// `None` → процесс не владеет навыками → курация не запускается (см. `sweep`).
    skills_root: Option<PathBuf>,
    /// owner-gated `ai.skills.learning_enabled` (default OFF). Конструируется из конфига; смена требует
    /// рестарта (как регистрация `skill_save`-tool в SL-7d).
    learning_enabled: bool,
}

impl SkillCuratorHandler {
    pub fn new(
        reader: ReadPool,
        writer: WriteActor,
        skills_root: Option<PathBuf>,
        learning_enabled: bool,
    ) -> Self {
        Self {
            reader,
            writer,
            skills_root,
            learning_enabled,
        }
    }

    /// Тестируемое ядро прогона (без `Job`/глобального времени): возвращает `(переходов, gc_удалено)`.
    ///
    /// Порядок: GC орфан-телеметрии ПЕРЕД lifecycle-разбором (чтобы не тратить переходы на уже-мёртвые),
    /// затем разбор agent-кандидатов давностью-первыми ([`agent_created_report`] сортирует по активности
    /// ASC). GC консервативен: чистим ТОЛЬКО когда каталог прочитан ЧИСТО (есть навыки И ноль ошибок) —
    /// иначе «при сомнении не разрушаем».
    async fn sweep(&self, now: i64, stale_after: i64, archive_after: i64) -> (usize, usize) {
        // Гейт fail-closed: выключено ИЛИ не владеем навыками → ничего не делаем.
        if !self.learning_enabled {
            return (0, 0);
        }
        let Some(root) = &self.skills_root else {
            return (0, 0);
        };

        // Кандидаты curator'а (agent-строки) — фетчим ОДИН раз: источник и lifecycle-разбора, и набора
        // ЗАКРЕПЛённых имён для защиты от GC ниже. (Орфан в этом списке после GC → set_state no-op, безвреден.)
        let candidates = match agent_created_report(&self.reader).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "skill_curator: чтение agent_created_report не удалось");
                return (0, 0);
            }
        };

        // ── GC орфан-телеметрии (best-effort, консервативно) ──
        // `names()` = ТОЛЬКО успешно РАЗОБРАННЫЕ навыки; файл, что ЕСТЬ на диске, но не парсится
        // (битый/половинчатый YAML, дубль-имя, vendor hash-drift, разовый IO), попадает в `errors()`,
        // а НЕ в `names()`. `forget_orphans(NOT IN names)` снёс бы тогда телеметрию/lifecycle ЖИВОГО
        // (но непарсящегося прямо сейчас) навыка — необратимая потеря pinned/archived/провенанса.
        let catalog = discover_skills(root);
        let live = catalog.names();
        let gc = if live.is_empty() || !catalog.errors().is_empty() {
            // Пустой набор (пустой каталог НЕ отличим от сбоя чтения корня) ИЛИ ЛЮБАЯ ошибка обнаружения
            // ⇒ НЕ чистим (ревью SL-curator MAJOR: «present-but-unparseable ≠ deleted»). GC лишь когда
            // каталог прочитан ЧИСТО; орфаны (если есть) доберёт следующий прогон по чистому каталогу.
            0
        } else {
            // Защита намерения владельца (ревью SL-curator #2): ЗАКРЕПЛённые agent-строки НИКОГДА не
            // считаем орфанами — добавляем их имена в «живой» набор. Так ручное переименование
            // frontmatter `name` (при сохранённой директории, чистый парс) или иной редкий рассинхрон
            // ключа не снесёт pin владельца. Цена: телеметрия закреплённого-но-удалённого-с-диска навыка
            // задержится до анпина (безвредный clutter).
            let mut protected = live;
            for rec in &candidates {
                if rec.pinned {
                    protected.push(rec.skill_name.clone());
                }
            }
            usage::forget_orphans(&self.writer, &protected)
                .await
                .unwrap_or(0)
        };

        // ── Lifecycle-разбор agent-кандидатов ──
        let mut transitions = 0usize;
        for rec in &candidates {
            if transitions >= MAX_TRANSITIONS_PER_RUN {
                break;
            }
            let changed = match decide(rec, now, stale_after, archive_after) {
                CuratorAction::Keep => false,
                CuratorAction::MarkStale => {
                    set_state(&self.writer, &rec.skill_name, SkillState::Stale)
                        .await
                        .unwrap_or(false)
                }
                CuratorAction::Archive => archive(&self.writer, &rec.skill_name)
                    .await
                    .unwrap_or(false),
            };
            if changed {
                transitions += 1;
            }
        }
        (transitions, gc)
    }
}

#[async_trait]
impl JobHandler for SkillCuratorHandler {
    /// Фоновая гигиена — уступает интерактиву (S5 backpressure): курация не спешит, делается «на простое».
    fn defer_under_interactive(&self) -> bool {
        true
    }

    async fn handle(&self, _job: &Job) -> Result<(), String> {
        let (transitions, gc) = self
            .sweep(now_secs(), STALE_AFTER_SECS, ARCHIVE_AFTER_SECS)
            .await;
        if transitions > 0 || gc > 0 {
            tracing::info!(
                transitions,
                gc_removed = gc,
                "skill_curator: lifecycle-прогон применён"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, WriteActor};
    use rusqlite::params;
    use tempfile::TempDir;

    async fn temp_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join(".nexus/nexus.db"))
            .await
            .expect("open db");
        (db, dir)
    }

    /// Прямой seed строки `agent_skill_usage` с явными значениями.
    #[allow(clippy::too_many_arguments)]
    async fn seed(
        writer: &WriteActor,
        name: &str,
        created_by: Option<&str>,
        state: &str,
        pinned: bool,
        last_used_at: Option<i64>,
        created_at: i64,
    ) {
        let name = name.to_string();
        let created_by = created_by.map(|s| s.to_string());
        let state = state.to_string();
        writer
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_skill_usage(skill_name, last_used_at, created_at, created_by, state, pinned) \
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![name, last_used_at, created_at, created_by, state, pinned as i64],
                )
                .map(|_| ())
            })
            .await
            .unwrap();
    }

    fn rec(
        state: SkillState,
        pinned: bool,
        last_used_at: Option<i64>,
        created_at: i64,
    ) -> UsageRecord {
        UsageRecord {
            skill_name: "x".into(),
            use_count: 0,
            view_count: 0,
            save_count: 0,
            patch_count: 0,
            last_used_at,
            last_viewed_at: None,
            last_saved_at: None,
            last_patched_at: None,
            created_at,
            created_by: Some("agent".into()),
            state: Some(state),
            pinned,
            archived_at: None,
        }
    }

    // ── decide(): чистая логика + «eval-гейт» точности ──

    #[test]
    fn decide_active_to_stale_after_idle() {
        // last_used=0, now=100, stale_after=50 → idle=100 ≥ 50 → MarkStale.
        let r = rec(SkillState::Active, false, Some(0), 0);
        assert_eq!(decide(&r, 100, 50, 200), CuratorAction::MarkStale);
        // now=40 → idle=40 < 50 → Keep.
        assert_eq!(decide(&r, 40, 50, 200), CuratorAction::Keep);
    }

    #[test]
    fn decide_stale_to_archive_after_idle() {
        let r = rec(SkillState::Stale, false, Some(0), 0);
        assert_eq!(decide(&r, 200, 50, 200), CuratorAction::Archive);
        assert_eq!(decide(&r, 150, 50, 200), CuratorAction::Keep);
    }

    /// ЕVAL-ГЕЙТ (precision = 1.0): pinned НИКОГДА не переходит, в любом состоянии и при любом простое.
    #[test]
    fn decide_pinned_never_transitions() {
        for state in [SkillState::Active, SkillState::Stale, SkillState::Archived] {
            let r = rec(state, /*pinned*/ true, Some(0), 0);
            assert_eq!(
                decide(&r, i64::MAX / 2, 1, 1),
                CuratorAction::Keep,
                "pinned ({state:?}) неприкосновенен даже при огромном простое"
            );
        }
    }

    /// ЕVAL-ГЕЙТ (precision = 1.0): активный-в-пределах-порога навык НИКОГДА не архивируется/устаревает.
    #[test]
    fn decide_recent_active_never_archived() {
        let r = rec(SkillState::Active, false, Some(95), 0); // idle=5
        assert_eq!(decide(&r, 100, 50, 200), CuratorAction::Keep);
    }

    /// fail-closed: Archived (терминально) и неизвестное состояние (None) → Keep (не трогаем).
    #[test]
    fn decide_archived_and_unknown_state_are_kept() {
        let archived = rec(SkillState::Archived, false, Some(0), 0);
        assert_eq!(decide(&archived, i64::MAX / 2, 1, 1), CuratorAction::Keep);
        let mut unknown = rec(SkillState::Active, false, Some(0), 0);
        unknown.state = None; // как будто в БД новее появилось неизвестное значение
        assert_eq!(decide(&unknown, i64::MAX / 2, 1, 1), CuratorAction::Keep);
    }

    /// Двухшаговость: долго-неактивный active НЕ перепрыгивает в archived за один прогон — сначала stale.
    #[test]
    fn decide_active_never_jumps_to_archive() {
        let r = rec(SkillState::Active, false, Some(0), 0);
        // Простой огромный (> archive_after), но active-арм проверяет лишь stale-порог:
        assert_eq!(decide(&r, 10_000, 50, 200), CuratorAction::MarkStale);
    }

    // ── sweep(): интеграция с БД ──

    /// Гейт: learning_enabled=false → полный NOOP (ни переходов, ни GC), даже при готовых кандидатах.
    #[tokio::test]
    async fn sweep_noop_when_learning_disabled() {
        let (db, dir) = temp_db().await;
        let w = db.writer();
        seed(w, "old", Some("agent"), "active", false, Some(0), 0).await;
        let h = SkillCuratorHandler::new(
            db.reader().clone(),
            w.clone(),
            Some(dir.path().to_path_buf()),
            /*learning*/ false,
        );
        let (t, gc) = h.sweep(10_000_000, 50, 200).await;
        assert_eq!((t, gc), (0, 0), "OFF → NOOP");
        let r = usage::get_record(db.reader(), "old")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            r.state,
            Some(SkillState::Active),
            "состояние не тронуто при OFF"
        );
    }

    /// Гейт: skills_root=None → NOOP (процесс не владеет навыками).
    #[tokio::test]
    async fn sweep_noop_without_skills_root() {
        let (db, _d) = temp_db().await;
        let w = db.writer();
        seed(w, "old", Some("agent"), "active", false, Some(0), 0).await;
        let h = SkillCuratorHandler::new(db.reader().clone(), w.clone(), None, true);
        let (t, gc) = h.sweep(10_000_000, 50, 200).await;
        assert_eq!((t, gc), (0, 0));
    }

    /// Полный lifecycle через sweep: давний active → stale; уже-stale-и-давний → archived; свежий и
    /// pinned — нетронуты; vendor (не-agent) — нетронут.
    #[tokio::test]
    async fn sweep_applies_lifecycle_only_to_eligible() {
        let (db, dir) = temp_db().await;
        let w = db.writer();
        // Создаём skills-каталог на диске с ОДНИМ навыком, чтобы live-набор был НЕ пуст (GC не нуль-кейс)
        // и не сносил телеметрию наших seed'ов. Все seed-имена попадут в «живой» набор иначе будут GC'нуты.
        // Проще: даём live-набор, содержащий ВСЕ наши имена, создав файлы — но discover требует валидный
        // frontmatter. Вместо этого ставим один реальный навык + проверяем, что GC снёс лишь отсутствующих.
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("keepall")).unwrap();
        std::fs::write(
            skills_dir.join("keepall/SKILL.md"),
            "---\nname: keepall\ndescription: marker\n---\nbody\n",
        )
        .unwrap();

        // Seed-навыки (НЕ на диске → их телеметрия орфанна и была бы GC'нута — поэтому для lifecycle-теста
        // создаём их ТОЖЕ на диске, включая vendorX: иначе GC снёс бы его строку до проверки «не тронут»).
        for n in [
            "agent-stale-cand",
            "agent-archive-cand",
            "agent-fresh",
            "agent-pinned",
            "vendorX",
        ] {
            std::fs::create_dir_all(skills_dir.join(n)).unwrap();
            std::fs::write(
                skills_dir.join(format!("{n}/SKILL.md")),
                format!("---\nname: {n}\ndescription: d\n---\nb\n"),
            )
            .unwrap();
        }

        seed(
            w,
            "agent-stale-cand",
            Some("agent"),
            "active",
            false,
            Some(0),
            0,
        )
        .await; // idle big → stale
        seed(
            w,
            "agent-archive-cand",
            Some("agent"),
            "stale",
            false,
            Some(0),
            0,
        )
        .await; // idle big → archive
        seed(
            w,
            "agent-fresh",
            Some("agent"),
            "active",
            false,
            Some(9_999_950),
            0,
        )
        .await; // idle=50 < stale → keep
        seed(w, "agent-pinned", Some("agent"), "active", true, Some(0), 0).await; // pinned → keep
        seed(w, "vendorX", Some("vendor"), "active", false, Some(0), 0).await; // не-agent → keep

        let h = SkillCuratorHandler::new(
            db.reader().clone(),
            w.clone(),
            Some(skills_dir.clone()),
            true,
        );
        // now=10_000_000, stale_after=100, archive_after=200; idle для seed(last_used=0) = 10_000_000.
        let (transitions, _gc) = h.sweep(10_000_000, 100, 200).await;
        assert_eq!(transitions, 2, "stale-cand→stale + archive-cand→archived");

        let st = |name: &str| {
            let reader = db.reader().clone();
            let name = name.to_string();
            async move { usage::get_record(&reader, &name).await.unwrap().unwrap() }
        };
        assert_eq!(st("agent-stale-cand").await.state, Some(SkillState::Stale));
        let arch = st("agent-archive-cand").await;
        assert_eq!(arch.state, Some(SkillState::Archived));
        assert!(arch.archived_at.is_some(), "archived_at проставлен");
        assert_eq!(
            st("agent-fresh").await.state,
            Some(SkillState::Active),
            "свежий не тронут"
        );
        assert_eq!(
            st("agent-pinned").await.state,
            Some(SkillState::Active),
            "pinned не тронут"
        );
        assert_eq!(
            st("vendorX").await.state,
            Some(SkillState::Active),
            "vendor (не-agent) не тронут"
        );
    }

    /// НИКОГДА-DELETE: прогон curator'а НЕ удаляет строку ЖИВОГО (на диске) навыка — лишь меняет state.
    /// Доказывает, что архивация обратима (строка цела), а forget_orphans не задел живого.
    #[tokio::test]
    async fn sweep_never_deletes_live_skill_row() {
        let (db, dir) = temp_db().await;
        let w = db.writer();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("live")).unwrap();
        std::fs::write(
            skills_dir.join("live/SKILL.md"),
            "---\nname: live\ndescription: d\n---\nb\n",
        )
        .unwrap();
        seed(w, "live", Some("agent"), "stale", false, Some(0), 0).await;

        let h = SkillCuratorHandler::new(db.reader().clone(), w.clone(), Some(skills_dir), true);
        h.sweep(10_000_000, 100, 200).await;

        let r = usage::get_record(db.reader(), "live").await.unwrap();
        assert!(r.is_some(), "строка живого навыка НЕ удалена");
        assert_eq!(
            r.unwrap().state,
            Some(SkillState::Archived),
            "лишь заархивирован (обратимо)"
        );
    }

    /// GC консервативен: пустой live-набор (несуществующий skills_root → discover вернёт пусто) НЕ чистит
    /// телеметрию (защита от транзиентного сбоя чтения).
    #[tokio::test]
    async fn sweep_gc_skipped_on_empty_live_set() {
        let (db, dir) = temp_db().await;
        let w = db.writer();
        // Телеметрия есть, но skills_root указывает на НЕсуществующий путь → discover_skills → пусто.
        usage::bump_use(w, "orphan-ish").await.unwrap();
        let missing = dir.path().join("does-not-exist");
        let h = SkillCuratorHandler::new(db.reader().clone(), w.clone(), Some(missing), true);
        let (_t, gc) = h.sweep(10_000_000, 100, 200).await;
        assert_eq!(gc, 0, "пустой live → GC пропущен");
        assert!(
            usage::get_record(db.reader(), "orphan-ish")
                .await
                .unwrap()
                .is_some(),
            "телеметрия НЕ снесена при пустом live (fail-closed)"
        );
    }

    /// GC орфанов: при НЕпустом live-наборе строка скилла, отсутствующего на диске, чистится; живой — нет.
    #[tokio::test]
    async fn sweep_gc_removes_orphans_when_live_nonempty() {
        let (db, dir) = temp_db().await;
        let w = db.writer();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("ondisk")).unwrap();
        std::fs::write(
            skills_dir.join("ondisk/SKILL.md"),
            "---\nname: ondisk\ndescription: d\n---\nb\n",
        )
        .unwrap();
        // Телеметрия для живого (ondisk) и для исчезнувшего (ghost).
        usage::bump_use(w, "ondisk").await.unwrap();
        usage::bump_use(w, "ghost").await.unwrap();

        let h = SkillCuratorHandler::new(db.reader().clone(), w.clone(), Some(skills_dir), true);
        let (_t, gc) = h.sweep(10_000_000, 100, 200).await;
        assert_eq!(gc, 1, "снесён только ghost");
        assert!(usage::get_record(db.reader(), "ghost")
            .await
            .unwrap()
            .is_none());
        assert!(
            usage::get_record(db.reader(), "ondisk")
                .await
                .unwrap()
                .is_some(),
            "живой не тронут"
        );
    }

    /// MAJOR-регрессия (ревью SL-curator): ЧАСТИЧНЫЙ сбой разбора каталога (один SKILL.md на диске, но
    /// непарсящийся) ⇒ GC ПОЛНОСТЬЮ пропущен, телеметрия/lifecycle присутствующего-но-битого навыка НЕ
    /// снесена. «present-but-unparseable ≠ deleted». Прежняя реализация (guard лишь на пустой live)
    /// снесла бы строку битого навыка (его имени нет в names() → forget_orphans счёл бы орфаном).
    #[tokio::test]
    async fn sweep_skips_gc_on_partial_parse_failure() {
        let (db, dir) = temp_db().await;
        let w = db.writer();
        let skills_dir = dir.path().join("skills");
        // Валидный навык на диске.
        std::fs::create_dir_all(skills_dir.join("good")).unwrap();
        std::fs::write(
            skills_dir.join("good/SKILL.md"),
            "---\nname: good\ndescription: d\n---\nb\n",
        )
        .unwrap();
        // Битый навык НА ДИСКЕ: нет frontmatter-блока → BadFrontmatter → попадает в errors(), НЕ в names().
        std::fs::create_dir_all(skills_dir.join("broken")).unwrap();
        std::fs::write(
            skills_dir.join("broken/SKILL.md"),
            "нет никакого frontmatter тут, просто текст\n",
        )
        .unwrap();
        // Телеметрия/lifecycle для обоих + истинный орфан (ghost, не на диске).
        seed(w, "good", Some("agent"), "active", false, Some(900), 0).await;
        seed(w, "broken", Some("agent"), "active", true, Some(900), 0).await; // pinned agent-навык
        usage::bump_use(w, "ghost").await.unwrap();

        let h = SkillCuratorHandler::new(db.reader().clone(), w.clone(), Some(skills_dir), true);
        let (_t, gc) = h.sweep(10_000_000, 100, 200).await;
        assert_eq!(gc, 0, "есть ошибка разбора → GC полностью пропущен");
        assert!(
            usage::get_record(db.reader(), "broken")
                .await
                .unwrap()
                .is_some(),
            "строка ЖИВОГО-но-непарсящегося навыка СОХРАНЕНА (pinned/провенанс не потеряны)"
        );
        assert!(
            usage::get_record(db.reader(), "good")
                .await
                .unwrap()
                .is_some(),
            "валидный навык сохранён"
        );
        // Истинный орфан тоже уцелел (GC скипнут целиком) — доберётся на чистом прогоне.
        assert!(
            usage::get_record(db.reader(), "ghost")
                .await
                .unwrap()
                .is_some(),
            "при сомнении не разрушаем — даже истинный орфан ждёт чистого каталога"
        );
    }

    /// #2 защита намерения владельца: ЗАКРЕПЛённый agent-навык, отсутствующий в «живом» наборе (имитация
    /// ручного переименования frontmatter / рассинхрона ключа), НЕ снимается GC даже на чистом каталоге;
    /// незакреплённый истинный орфан — снимается.
    #[tokio::test]
    async fn sweep_gc_preserves_pinned_even_if_absent() {
        let (db, dir) = temp_db().await;
        let w = db.writer();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("present")).unwrap();
        std::fs::write(
            skills_dir.join("present/SKILL.md"),
            "---\nname: present\ndescription: d\n---\nb\n",
        )
        .unwrap();
        // Закреплённый agent-навык, которого НЕТ в live-наборе (не на диске под этим именем).
        seed(
            w,
            "pinned-absent",
            Some("agent"),
            "active",
            true,
            Some(900),
            0,
        )
        .await;
        // Истинный НЕзакреплённый орфан (не на диске) — он и должен быть снят.
        usage::bump_use(w, "ghost").await.unwrap();

        let h = SkillCuratorHandler::new(db.reader().clone(), w.clone(), Some(skills_dir), true);
        let (_t, gc) = h.sweep(10_000_000, 100, 200).await;
        assert_eq!(gc, 1, "снят только незакреплённый орфан");
        assert!(
            usage::get_record(db.reader(), "pinned-absent")
                .await
                .unwrap()
                .is_some(),
            "закреплённый agent-навык защищён от GC (намерение владельца)"
        );
        assert!(
            usage::get_record(db.reader(), "ghost")
                .await
                .unwrap()
                .is_none(),
            "незакреплённый истинный орфан снят"
        );
    }

    /// Анти-flood: не более [`MAX_TRANSITIONS_PER_RUN`] переходов за прогон.
    #[tokio::test]
    async fn sweep_caps_transitions_per_run() {
        let (db, dir) = temp_db().await;
        let w = db.writer();
        let skills_dir = dir.path().join("skills");
        // MAX+5 давних active agent-навыков, все на диске (чтобы GC не снёс).
        let total = MAX_TRANSITIONS_PER_RUN + 5;
        for i in 0..total {
            let n = format!("s{i}");
            std::fs::create_dir_all(skills_dir.join(&n)).unwrap();
            std::fs::write(
                skills_dir.join(format!("{n}/SKILL.md")),
                format!("---\nname: {n}\ndescription: d\n---\nb\n"),
            )
            .unwrap();
            seed(w, &n, Some("agent"), "active", false, Some(0), 0).await;
        }
        let h = SkillCuratorHandler::new(db.reader().clone(), w.clone(), Some(skills_dir), true);
        let (transitions, _gc) = h.sweep(10_000_000, 100, 200).await;
        assert_eq!(
            transitions, MAX_TRANSITIONS_PER_RUN,
            "переходы ограничены капом за прогон"
        );
    }
}
