//! Тесты MEM-8a (бэкенд консолидации): парсер op (fail-closed), `plan` (короткое замыкание точного
//! дубля / пустого индекса; маппинг op→PlanOp), `apply` (стейт-машина UPDATE/supersede/NOOP/ADD,
//! optimistic-чек гонки, op_group, soft-supersede + восстановление). LLM — детерминированный стаб.

use super::*;
use crate::ai::{AiError, AiResult, MockEmbedder};
use crate::db::Database;
use crate::memory::{add, fact_history, index_fact, list, SOURCE_AUTO, SOURCE_EXPLICIT};
use crate::vector::VectorIndex;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

async fn open() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path().join(".nexus/nexus.db"))
        .await
        .unwrap();
    (dir, db)
}

/// Стаб основной модели: возвращает фикс-ответ, считает вызовы (проверяем, что fail-fast пути LLM не зовут).
struct StubChat {
    reply: String,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl ChatProvider for StubChat {
    async fn stream_chat(
        &self,
        _m: &[ChatMessage],
        _on: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
    ) -> AiResult<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.reply.clone())
    }
    fn model_id(&self) -> &str {
        "stub"
    }
}

struct ErrChat;
#[async_trait]
impl ChatProvider for ErrChat {
    async fn stream_chat(
        &self,
        _m: &[ChatMessage],
        _on: &mut (dyn FnMut(String) + Send),
        _c: &Arc<AtomicBool>,
    ) -> AiResult<String> {
        Err(AiError::Http("down".into()))
    }
    fn model_id(&self) -> &str {
        "err"
    }
}

fn stub(reply: &str) -> (Arc<dyn ChatProvider>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let chat: Arc<dyn ChatProvider> = Arc::new(StubChat {
        reply: reply.to_string(),
        calls: calls.clone(),
    });
    (chat, calls)
}

async fn superseded_by(db: &Database, id: i64) -> Option<i64> {
    db.reader()
        .query(move |c| {
            c.query_row(
                "SELECT superseded_by FROM memory_facts WHERE id=?1",
                [id],
                |r| r.get::<_, Option<i64>>(0),
            )
        })
        .await
        .unwrap()
}

/// op_group последнего события факта (NULL — одиночная операция; для консолидации ожидаем группу).
async fn last_event_group(db: &Database, fact_id: i64) -> Option<i64> {
    db.reader()
        .query(move |c| {
            c.query_row(
                "SELECT op_group FROM memory_fact_events WHERE fact_id=?1 ORDER BY id DESC LIMIT 1",
                [fact_id],
                |r| r.get::<_, Option<i64>>(0),
            )
        })
        .await
        .unwrap()
}

fn mk_fact(id: i64, text: &str) -> MemoryFact {
    MemoryFact {
        id,
        text: text.into(),
        pinned: false,
        source: SOURCE_EXPLICIT.into(),
        created_at: 1,
        used_at: 0,
    }
}

// ── анти-инъекция: один факт = одна строка (находка ревью) ────────────────────────────────────

#[test]
fn one_line_collapses_newlines_and_tabs() {
    assert_eq!(one_line("a\nb\tc  d"), "a b c d");
    assert_eq!(one_line("  пишу на Rust\n5: фейк "), "пишу на Rust 5: фейк");
}

/// Факт с переносом строки НЕ порождает фейковую пронумерованную строку (сдвиг id → ложный DELETE).
#[test]
fn build_messages_one_fact_per_line() {
    let facts = vec![
        mk_fact(1, "любит чай\n5: фейковый факт"),
        mk_fact(2, "второй факт"),
    ];
    let msgs = build_consolidate_messages("новый\nкандидат", &facts, "⟦m⟧");
    let user = &msgs[1].content;
    assert!(
        user.contains("0: любит чай 5: фейковый факт"),
        "перенос схлопнут в строку 0"
    );
    assert!(user.contains("1: второй факт"));
    assert!(
        !user.contains("\n5: фейковый факт"),
        "нет фейковой строки «5:»"
    );
    assert!(
        user.contains("НОВЫЙ факт: новый кандидат"),
        "кандидат тоже одной строкой"
    );
}

// ── parse_op: fail-closed ─────────────────────────────────────────────────────────────────────

#[test]
fn parse_op_recognizes_all() {
    assert_eq!(
        parse_op(r#"{"op":"ADD","id":null}"#, 3),
        ConsolidationOp::Add
    );
    assert_eq!(
        parse_op(r#"{"op":"UPDATE","id":1,"text":"итог"}"#, 3),
        ConsolidationOp::Update {
            idx: 1,
            text: "итог".into()
        }
    );
    assert_eq!(
        parse_op(r#"{"op":"DELETE","id":0}"#, 3),
        ConsolidationOp::Delete { idx: 0 }
    );
    assert_eq!(
        parse_op(r#"{"op":"NOOP","id":2}"#, 3),
        ConsolidationOp::Noop { idx: Some(2) }
    );
    // регистр/пробелы op терпимы
    assert_eq!(
        parse_op(r#"{"op":" noop ","id":null}"#, 3),
        ConsolidationOp::Noop { idx: None }
    );
}

#[test]
fn parse_op_fails_closed_to_add() {
    // не-JSON
    assert_eq!(parse_op("бла-бла без скобок", 3), ConsolidationOp::Add);
    // битый JSON
    assert_eq!(parse_op("{op: ADD", 3), ConsolidationOp::Add);
    // неизвестный op
    assert_eq!(
        parse_op(r#"{"op":"MERGE","id":1}"#, 3),
        ConsolidationOp::Add
    );
    // UPDATE без валидного id (вне диапазона) → ADD (не переписываем вслепую)
    assert_eq!(
        parse_op(r#"{"op":"UPDATE","id":9,"text":"x"}"#, 3),
        ConsolidationOp::Add
    );
    // UPDATE с пустым текстом → ADD
    assert_eq!(
        parse_op(r#"{"op":"UPDATE","id":1,"text":"   "}"#, 3),
        ConsolidationOp::Add
    );
    // DELETE без id → ADD (не удаляем вслепую)
    assert_eq!(
        parse_op(r#"{"op":"DELETE","id":null}"#, 3),
        ConsolidationOp::Add
    );
    // отрицательный id невалиден
    assert_eq!(
        parse_op(r#"{"op":"DELETE","id":-1}"#, 3),
        ConsolidationOp::Add
    );
}

#[test]
fn parse_op_tolerates_fenced_json() {
    let fenced = "Решение:\n```json\n{\"op\":\"DELETE\",\"id\":0}\n```";
    assert_eq!(parse_op(fenced, 2), ConsolidationOp::Delete { idx: 0 });
}

// ── plan: короткие замыкания + маппинг ────────────────────────────────────────────────────────

/// Точный дубль среди живых → NOOP без LLM (нижний слой дедупа).
#[tokio::test]
async fn plan_exact_dup_is_noop_without_llm() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("m.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };
    let id = add(db.writer(), "пишу на Rust", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let (chat, calls) = stub(r#"{"op":"DELETE","id":0}"#);
    let p = plan(
        db.reader(),
        &vectors,
        &emb,
        &chat,
        "  пишу на Rust  ",
        SOURCE_EXPLICIT,
    )
    .await
    .unwrap();
    assert_eq!(p.op, PlanOp::Noop { covered_by: id });
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "точный дубль — LLM не зван"
    );
}

/// Пустой индекс → ADD без LLM.
#[tokio::test]
async fn plan_empty_index_is_add_without_llm() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("m.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };
    let (chat, calls) = stub(r#"{"op":"DELETE","id":0}"#);
    let p = plan(
        db.reader(),
        &vectors,
        &emb,
        &chat,
        "новый факт",
        SOURCE_EXPLICIT,
    )
    .await
    .unwrap();
    assert_eq!(p.op, PlanOp::Add);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "пустой индекс — LLM не зван"
    );
}

/// Близкий факт в индексе + LLM решает DELETE → Supersede с реальным target_id.
#[tokio::test]
async fn plan_neighbor_delete_maps_to_supersede() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("m.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };
    // Существующий факт; кандидат — почти тот же текст (высокая косинусная близость mock-эмбеддера).
    let old = add(
        db.writer(),
        "дедлайн проекта в пятницу точно",
        SOURCE_EXPLICIT,
    )
    .await
    .unwrap()
    .unwrap()
    .0;
    index_fact(&vectors, &emb, old, "дедлайн проекта в пятницу точно")
        .await
        .unwrap();
    let (chat, calls) = stub(r#"{"op":"DELETE","id":0}"#);
    let p = plan(
        db.reader(),
        &vectors,
        &emb,
        &chat,
        "дедлайн проекта в пятницу теперь среда",
        SOURCE_EXPLICIT,
    )
    .await
    .unwrap();
    assert!(calls.load(Ordering::SeqCst) >= 1, "близкий факт → LLM зван");
    assert_eq!(
        p.op,
        PlanOp::Supersede {
            target_id: old,
            old_text: "дедлайн проекта в пятницу точно".into(),
            target_source: "explicit".into(),
        },
        "DELETE замаплен в supersede по реальному id"
    );
}

/// Близкий факт + LLM UPDATE → PlanOp::Update с объединённым текстом.
#[tokio::test]
async fn plan_neighbor_update_maps_to_update() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("m.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };
    let old = add(
        db.writer(),
        "пользователь любит зелёный чай",
        SOURCE_EXPLICIT,
    )
    .await
    .unwrap()
    .unwrap()
    .0;
    index_fact(&vectors, &emb, old, "пользователь любит зелёный чай")
        .await
        .unwrap();
    let (chat, _) =
        stub(r#"{"op":"UPDATE","id":0,"text":"пользователь любит зелёный чай без сахара"}"#);
    let p = plan(
        db.reader(),
        &vectors,
        &emb,
        &chat,
        "пользователь любит зелёный чай без сахара совсем",
        SOURCE_EXPLICIT,
    )
    .await
    .unwrap();
    assert_eq!(
        p.op,
        PlanOp::Update {
            target_id: old,
            old_text: "пользователь любит зелёный чай".into(),
            new_text: "пользователь любит зелёный чай без сахара".into(),
            target_source: "explicit".into(),
        }
    );
}

/// LLM-ошибка на близком факте → fail-closed ADD (не теряем кандидата).
#[tokio::test]
async fn plan_llm_error_fails_closed_to_add() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("m.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };
    let old = add(
        db.writer(),
        "город проживания Тбилиси давно",
        SOURCE_EXPLICIT,
    )
    .await
    .unwrap()
    .unwrap()
    .0;
    index_fact(&vectors, &emb, old, "город проживания Тбилиси давно")
        .await
        .unwrap();
    let chat: Arc<dyn ChatProvider> = Arc::new(ErrChat);
    let p = plan(
        db.reader(),
        &vectors,
        &emb,
        &chat,
        "город проживания Тбилиси давно уже",
        SOURCE_EXPLICIT,
    )
    .await
    .unwrap();
    assert_eq!(p.op, PlanOp::Add, "ошибка LLM → ADD");
}

// ── apply: стейт-машина ───────────────────────────────────────────────────────────────────────

fn add_plan(text: &str) -> ConsolidationPlan {
    ConsolidationPlan {
        candidate: text.into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Add,
    }
}

/// ADD: вставляет новый факт.
#[tokio::test]
async fn apply_add_inserts() {
    let (_d, db) = open().await;
    let out = apply(
        db.writer(),
        add_plan("совсем новый факт"),
        ConsolidationChoice::Accept,
    )
    .await
    .unwrap();
    match out {
        ConsolidationOutcome::Add { inserted, .. } => assert!(inserted),
        o => panic!("ожидался Add, получено {o:?}"),
    }
    assert_eq!(list(db.reader()).await.unwrap().len(), 1);
}

/// UPDATE Accept: дополняет целевой факт, пишет update-событие с op_group; список содержит новый текст.
#[tokio::test]
async fn apply_update_merges_and_logs() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "пьёт кофе", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let plan = ConsolidationPlan {
        candidate: "пьёт кофе по утрам".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Update {
            target_id: old,
            old_text: "пьёт кофе".into(),
            new_text: "пьёт кофе по утрам".into(),
            target_source: "explicit".into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    match out {
        ConsolidationOutcome::Update { id, op_group, .. } => {
            assert_eq!(id, old);
            assert!(op_group >= 1);
        }
        o => panic!("ожидался Update, получено {o:?}"),
    }
    let facts = list(db.reader()).await.unwrap();
    assert_eq!(
        facts.len(),
        1,
        "новый факт НЕ заведён — текст влит в старый"
    );
    assert_eq!(facts[0].text, "пьёт кофе по утрам");
    let hist = fact_history(db.reader(), old).await.unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].event, "update");
    assert!(
        last_event_group(&db, old).await.is_some(),
        "событие консолидации записано с op_group"
    );
}

/// UPDATE с УСТАРЕВШИМ снимком (текст цели изменился с момента plan) → деградация в ADD (не портим чужое).
#[tokio::test]
async fn apply_update_stale_target_degrades_to_add() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "исходный текст факта", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    // Гонка: кто-то поправил факт после того, как мы посчитали plan на старом тексте.
    crate::memory::edit(db.writer(), old, "уже другой текст")
        .await
        .unwrap();
    let plan = ConsolidationPlan {
        candidate: "кандидат на слияние".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Update {
            target_id: old,
            old_text: "исходный текст факта".into(), // снимок устарел
            new_text: "слитый текст".into(),
            target_source: "explicit".into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    assert!(
        matches!(out, ConsolidationOutcome::Add { inserted: true, .. }),
        "устаревший снимок → ADD кандидата, цель не тронута"
    );
    let facts = list(db.reader()).await.unwrap();
    assert!(
        facts.iter().any(|f| f.text == "уже другой текст"),
        "цель цела"
    );
    assert!(
        facts.iter().any(|f| f.text == "кандидат на слияние"),
        "кандидат добавлен"
    );
}

/// SUPERSEDE Accept: добавляет новый, помечает старый супридённым (вне списка), пишет supersede-событие.
#[tokio::test]
async fn apply_supersede_adds_new_and_retires_old() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "дедлайн пятница", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let plan = ConsolidationPlan {
        candidate: "дедлайн среда".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Supersede {
            target_id: old,
            old_text: "дедлайн пятница".into(),
            target_source: "explicit".into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    let (new_id, supe) = match out {
        ConsolidationOutcome::Supersede {
            id, superseded_id, ..
        } => (id, superseded_id),
        o => panic!("ожидался Supersede, получено {o:?}"),
    };
    assert_eq!(supe, old);
    // Список: только новый факт (старый супридён).
    let facts = list(db.reader()).await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].id, new_id);
    assert_eq!(facts[0].text, "дедлайн среда");
    // Старый — помечен, но физически жив (откатываемо).
    assert_eq!(superseded_by(&db, old).await, Some(new_id));
    let hist = fact_history(db.reader(), old).await.unwrap();
    assert_eq!(hist[0].event, "supersede");
    assert_eq!(hist[0].new_text.as_deref(), Some("дедлайн среда"));
}

/// SUPERSEDE KeepSeparate: пользователь оставил оба — новый добавлен, старый ЖИВ.
#[tokio::test]
async fn apply_supersede_keep_separate_keeps_both() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "дедлайн пятница", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let plan = ConsolidationPlan {
        candidate: "дедлайн среда".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Supersede {
            target_id: old,
            old_text: "дедлайн пятница".into(),
            target_source: "explicit".into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::KeepSeparate)
        .await
        .unwrap();
    assert!(matches!(
        out,
        ConsolidationOutcome::Add { inserted: true, .. }
    ));
    assert_eq!(list(db.reader()).await.unwrap().len(), 2, "оба факта живы");
    assert_eq!(superseded_by(&db, old).await, None, "старый не тронут");
}

/// SUPERSEDE с уже-супридённой целью (гонка двух консолидаций) → optimistic-чек деградирует в ADD.
#[tokio::test]
async fn apply_supersede_already_retired_degrades_to_add() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "старый факт гонки", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    // Эмулируем: цель уже супридена другим (как сделала бы параллельная консолидация).
    db.writer()
        .transaction(move |tx| {
            tx.execute(
                "UPDATE memory_facts SET superseded_by=999, superseded_at=1 WHERE id=?1",
                [old],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let plan = ConsolidationPlan {
        candidate: "новый факт гонки".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Supersede {
            target_id: old,
            old_text: "старый факт гонки".into(),
            target_source: "explicit".into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    assert!(
        matches!(out, ConsolidationOutcome::Add { inserted: true, .. }),
        "уже супридённая цель → ADD (не двойное замещение)"
    );
    assert_eq!(
        superseded_by(&db, old).await,
        Some(999),
        "чужое замещение цело"
    );
}

/// NOOP Accept — ничего не пишет; KeepSeparate — добавляет кандидата.
#[tokio::test]
async fn apply_noop_accept_writes_nothing() {
    let (_d, db) = open().await;
    add(db.writer(), "уже знаю это", SOURCE_EXPLICIT)
        .await
        .unwrap();
    let plan = ConsolidationPlan {
        candidate: "почти то же самое".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Noop { covered_by: 1 },
    };
    let out = apply(db.writer(), plan.clone(), ConsolidationChoice::Accept)
        .await
        .unwrap();
    assert_eq!(out, ConsolidationOutcome::Noop);
    assert_eq!(
        list(db.reader()).await.unwrap().len(),
        1,
        "NOOP ничего не добавил"
    );

    let out2 = apply(db.writer(), plan, ConsolidationChoice::KeepSeparate)
        .await
        .unwrap();
    assert!(matches!(
        out2,
        ConsolidationOutcome::Add { inserted: true, .. }
    ));
    assert_eq!(
        list(db.reader()).await.unwrap().len(),
        2,
        "KeepSeparate добавил"
    );
}

/// Re-добавление СУПРИДЁННОГО текста восстанавливает факт (а не возвращает «мёртвый» id).
#[tokio::test]
async fn apply_add_resurrects_superseded_text() {
    let (_d, db) = open().await;
    let a = add(db.writer(), "факт который вернётся", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    // Помечаем супридённым (как сделала бы консолидация).
    db.writer()
        .transaction(move |tx| {
            tx.execute(
                "UPDATE memory_facts SET superseded_by=42, superseded_at=1 WHERE id=?1",
                [a],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        list(db.reader()).await.unwrap().is_empty(),
        "супридён — не в списке"
    );
    // Re-добавляем тот же текст.
    let out = apply(
        db.writer(),
        add_plan("факт который вернётся"),
        ConsolidationChoice::Accept,
    )
    .await
    .unwrap();
    match out {
        ConsolidationOutcome::Add { id, inserted } => {
            assert_eq!(id, a, "тот же id");
            assert!(
                inserted,
                "восстановление считается inserted (ре-индексация)"
            );
        }
        o => panic!("ожидался Add, получено {o:?}"),
    }
    let facts = list(db.reader()).await.unwrap();
    assert_eq!(facts.len(), 1, "факт снова жив");
    assert_eq!(superseded_by(&db, a).await, None);
    assert!(
        fact_history(db.reader(), a)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event == "restore"),
        "восстановление залогировано"
    );
}

/// SUPERSEDE, где кандидат совпал с ДРУГИМ ЖИВЫМ фактом → деградация в ADD (не супридим target к
/// несвязанному курированному факту; находка ревью).
#[tokio::test]
async fn apply_supersede_candidate_is_other_live_fact_degrades() {
    let (_d, db) = open().await;
    let target = add(db.writer(), "дедлайн пятница", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    add(db.writer(), "дедлайн среда", SOURCE_EXPLICIT)
        .await
        .unwrap(); // уже живой факт
    let plan = ConsolidationPlan {
        candidate: "дедлайн среда".into(), // совпадает с уже живым
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Supersede {
            target_id: target,
            old_text: "дедлайн пятница".into(),
            target_source: "explicit".into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    assert!(
        matches!(
            out,
            ConsolidationOutcome::Add {
                inserted: false,
                ..
            }
        ),
        "кандидат-дубль живого → ADD, не supersede"
    );
    assert_eq!(
        superseded_by(&db, target).await,
        None,
        "target НЕ супридён к чужому"
    );
    assert_eq!(list(db.reader()).await.unwrap().len(), 2, "оба живы");
}

/// SUPERSEDE: новый факт и supersede старого — в ОДНОЙ op_group (групповой откат, §4.6; находка ревью).
#[tokio::test]
async fn apply_supersede_new_fact_shares_op_group() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "старый дедлайн", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let plan = ConsolidationPlan {
        candidate: "новый дедлайн".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Supersede {
            target_id: old,
            old_text: "старый дедлайн".into(),
            target_source: "explicit".into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    let new_id = match out {
        ConsolidationOutcome::Supersede { id, .. } => id,
        o => panic!("ожидался Supersede, получено {o:?}"),
    };
    let g_old = last_event_group(&db, old).await;
    let g_new = last_event_group(&db, new_id).await;
    assert!(g_old.is_some(), "supersede старого в группе");
    assert_eq!(g_old, g_new, "add нового и supersede старого — одна группа");
}

/// UPDATE без реальной правки (new==old) → Noop, без события/ре-эмбеда (находка ревью).
#[tokio::test]
async fn apply_update_noop_when_text_unchanged() {
    let (_d, db) = open().await;
    let id = add(db.writer(), "факт без изменений", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let plan = ConsolidationPlan {
        candidate: "факт без изменений".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Update {
            target_id: id,
            old_text: "факт без изменений".into(),
            new_text: "факт без изменений".into(),
            target_source: "explicit".into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    assert_eq!(out, ConsolidationOutcome::Noop);
    assert!(
        fact_history(db.reader(), id).await.unwrap().is_empty(),
        "пустой UPDATE не пишет событие"
    );
}

/// Точный дубль ЖИВОГО факта при ADD → inserted=false (MEM-5: undo не должен трогать чужой факт).
#[tokio::test]
async fn apply_add_live_duplicate_is_not_inserted() {
    let (_d, db) = open().await;
    add(db.writer(), "живой дубль", SOURCE_EXPLICIT)
        .await
        .unwrap();
    let out = apply(
        db.writer(),
        add_plan("живой дубль"),
        ConsolidationChoice::Accept,
    )
    .await
    .unwrap();
    assert!(matches!(
        out,
        ConsolidationOutcome::Add {
            inserted: false,
            ..
        }
    ));
    assert_eq!(list(db.reader()).await.unwrap().len(), 1);
}

// ── undo: откат группы консолидации (MEM-8c-b) ────────────────────────────────────────────────

/// Откат SUPERSEDE: старый факт возвращается живым, новый удаляется.
#[tokio::test]
async fn undo_supersede_restores_old_deletes_new() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "дедлайн пятница", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let plan = ConsolidationPlan {
        candidate: "дедлайн среда".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Supersede {
            target_id: old,
            old_text: "дедлайн пятница".into(),
            target_source: SOURCE_EXPLICIT.into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    let (new_id, group) = match out {
        ConsolidationOutcome::Supersede { id, op_group, .. } => (id, op_group),
        o => panic!("ожидался Supersede, получено {o:?}"),
    };
    assert_eq!(
        list(db.reader()).await.unwrap().len(),
        1,
        "до отката — только новый"
    );

    let u = undo(db.writer(), group).await.unwrap();
    assert!(u.reverted());
    let facts = list(db.reader()).await.unwrap();
    assert_eq!(facts.len(), 1, "старый вернулся, новый удалён");
    assert_eq!(facts[0].id, old);
    assert_eq!(facts[0].text, "дедлайн пятница");
    assert_eq!(superseded_by(&db, old).await, None, "старый снова жив");
    assert!(!facts.iter().any(|f| f.id == new_id), "новый удалён");
}

/// Откат UPDATE: текст факта возвращается к старому.
#[tokio::test]
async fn undo_update_reverts_text() {
    let (_d, db) = open().await;
    let id = add(db.writer(), "пьёт кофе", SOURCE_AUTO)
        .await
        .unwrap()
        .unwrap()
        .0;
    let plan = ConsolidationPlan {
        candidate: "пьёт кофе по утрам".into(),
        source: SOURCE_AUTO.into(),
        op: PlanOp::Update {
            target_id: id,
            old_text: "пьёт кофе".into(),
            new_text: "пьёт кофе по утрам".into(),
            target_source: SOURCE_AUTO.into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    let group = match out {
        ConsolidationOutcome::Update { op_group, .. } => op_group,
        o => panic!("ожидался Update, получено {o:?}"),
    };
    assert_eq!(
        list(db.reader()).await.unwrap()[0].text,
        "пьёт кофе по утрам"
    );

    let u = undo(db.writer(), group).await.unwrap();
    assert!(u.reverted());
    assert_eq!(
        list(db.reader()).await.unwrap()[0].text,
        "пьёт кофе",
        "текст откатан к старому"
    );
}

/// Откат SUPERSEDE, где новый факт ОТРЕДАКТИРОВАН после консолидации → новый НЕ удаляется (правка юзера
/// цела), старый возвращается → оба живы (безопасная частичная отмена).
#[tokio::test]
async fn undo_supersede_skips_edited_new_fact() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "старый факт", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let plan = ConsolidationPlan {
        candidate: "новый факт".into(),
        source: SOURCE_EXPLICIT.into(),
        op: PlanOp::Supersede {
            target_id: old,
            old_text: "старый факт".into(),
            target_source: SOURCE_EXPLICIT.into(),
        },
    };
    let out = apply(db.writer(), plan, ConsolidationChoice::Accept)
        .await
        .unwrap();
    let (new_id, group) = match out {
        ConsolidationOutcome::Supersede { id, op_group, .. } => (id, op_group),
        o => panic!("ожидался Supersede, получено {o:?}"),
    };
    // Юзер правит новый факт после авто-замещения.
    crate::memory::edit(db.writer(), new_id, "новый факт, дополненный юзером")
        .await
        .unwrap();

    let u = undo(db.writer(), group).await.unwrap();
    assert!(u.reverted(), "старый всё равно восстановлен");
    let facts = list(db.reader()).await.unwrap();
    assert!(
        facts.iter().any(|f| f.id == old && f.text == "старый факт"),
        "старый вернулся"
    );
    assert!(
        facts
            .iter()
            .any(|f| f.id == new_id && f.text == "новый факт, дополненный юзером"),
        "правка юзера НЕ потеряна"
    );
    assert_eq!(facts.len(), 2, "оба живы");
}

/// Откат неизвестной группы — no-op.
#[tokio::test]
async fn undo_unknown_group_is_noop() {
    let (_d, db) = open().await;
    add(db.writer(), "факт", SOURCE_EXPLICIT).await.unwrap();
    let u = undo(db.writer(), 99999).await.unwrap();
    assert!(!u.reverted());
    assert_eq!(list(db.reader()).await.unwrap().len(), 1);
}

/// Откат идемпотентен: повторный undo той же группы ничего не меняет.
#[tokio::test]
async fn undo_is_idempotent() {
    let (_d, db) = open().await;
    let old = add(db.writer(), "старый", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let out = apply(
        db.writer(),
        ConsolidationPlan {
            candidate: "новый".into(),
            source: SOURCE_EXPLICIT.into(),
            op: PlanOp::Supersede {
                target_id: old,
                old_text: "старый".into(),
                target_source: SOURCE_EXPLICIT.into(),
            },
        },
        ConsolidationChoice::Accept,
    )
    .await
    .unwrap();
    let group = match out {
        ConsolidationOutcome::Supersede { op_group, .. } => op_group,
        o => panic!("{o:?}"),
    };
    assert!(undo(db.writer(), group).await.unwrap().reverted());
    let snapshot = list(db.reader()).await.unwrap();
    assert!(
        !undo(db.writer(), group).await.unwrap().reverted(),
        "второй откат — no-op"
    );
    assert_eq!(
        list(db.reader()).await.unwrap(),
        snapshot,
        "состояние не изменилось"
    );
}
