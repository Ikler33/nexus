//! Тесты MEM-1 (бэкенд памяти агента): CRUD + дедуп + эмбеддинг-поиск `context_facts` (AC-MEM-1..4, 9).

use super::*;
use crate::ai::MockEmbedder;
use crate::db::Database;
use crate::vector::VectorIndex;
use tempfile::TempDir;

async fn open() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path().join(".nexus/nexus.db"))
        .await
        .unwrap();
    (dir, db)
}

/// AC-MEM-1/2: add пишет факт, дедуп по точному тексту, пустой отклоняется; list — пины сверху.
#[tokio::test]
async fn add_dedup_and_list() {
    let (_d, db) = open().await;
    let id1 = add(db.writer(), "пишу на Rust", SOURCE_EXPLICIT)
        .await
        .unwrap();
    assert_eq!(
        id1.map(|(_, ins)| ins),
        Some(true),
        "первая запись — inserted"
    );
    let id2 = add(db.writer(), "дедлайн проекта X — пятница", SOURCE_AUTO)
        .await
        .unwrap();
    assert!(id2.is_some());
    // дубль (точный текст) — тот же id, inserted=false (MEM-5).
    let dup = add(db.writer(), "  пишу на Rust  ", SOURCE_EXPLICIT)
        .await
        .unwrap();
    assert_eq!(
        dup.map(|(id, _)| id),
        id1.map(|(id, _)| id),
        "дубль вернул существующий id (trim)"
    );
    assert_eq!(
        dup.map(|(_, ins)| ins),
        Some(false),
        "дубль — inserted=false"
    );
    // пустой — None.
    assert_eq!(
        add(db.writer(), "   ", SOURCE_EXPLICIT).await.unwrap(),
        None
    );

    let facts = list(db.reader()).await.unwrap();
    assert_eq!(facts.len(), 2, "ровно 2 факта (дубль не плодит)");
    assert_eq!(count(db.reader()).await.unwrap(), 2);
}

/// AC-MEM-2/3: пин поднимает факт наверх; edit меняет текст; delete убирает.
#[tokio::test]
async fn pin_edit_delete() {
    let (_d, db) = open().await;
    let a = add(db.writer(), "факт А", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let b = add(db.writer(), "факт Б", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;

    set_pinned(db.writer(), b, true).await.unwrap();
    let facts = list(db.reader()).await.unwrap();
    assert_eq!(facts[0].id, b, "пин — сверху");
    assert!(facts[0].pinned);

    edit(db.writer(), a, "факт А (уточнён)").await.unwrap();
    let facts = list(db.reader()).await.unwrap();
    assert!(facts.iter().any(|f| f.text == "факт А (уточнён)"));

    delete(db.writer(), a).await.unwrap();
    let facts = list(db.reader()).await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].id, b);
}

/// AC-MEM-3: edit пустым/whitespace — no-op (текст в БД не меняется). Команда поверх этого НЕ ре-эмбеддит
/// (иначе вектор факта затёрся бы embedding'ом пустой строки → рассинхрон индекса с БД).
#[tokio::test]
async fn edit_empty_is_noop() {
    let (_d, db) = open().await;
    let id = add(db.writer(), "исходный текст", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    edit(db.writer(), id, "   ").await.unwrap();
    let facts = list(db.reader()).await.unwrap();
    assert_eq!(
        facts[0].text, "исходный текст",
        "пустой edit не меняет текст"
    );
}

/// AC-MEM-4 (D2): context_facts = ВСЕ пины + top-k семантически близких; used_at обновляется.
#[tokio::test]
async fn context_facts_pins_plus_topk() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("mem.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };

    // 3 факта: пин (не про запрос), релевантный (байт-выровненный префикс с запросом — mock-эмбеддер
    // позиционно-чувствителен), нерелевантный.
    let pin = add(db.writer(), "меня зовут Артан", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let rel = add(db.writer(), "Nexus проект на Rust и Tauri", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let other = add(db.writer(), "погода сегодня солнечная", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    for (id, text) in [
        (pin, "меня зовут Артан"),
        (rel, "Nexus проект на Rust и Tauri"),
        (other, "погода сегодня солнечная"),
    ] {
        index_fact(&vectors, &emb, id, text).await.unwrap();
    }
    set_pinned(db.writer(), pin, true).await.unwrap();

    // Запрос = текст релевантного факта → cosine 1.0 с ним, детерминированно топ среди не-пинов.
    let facts = context_facts(
        db.reader(),
        db.writer(),
        &vectors,
        &emb,
        "Nexus проект на Rust и Tauri",
        1,
    )
    .await
    .unwrap();
    let ids: Vec<i64> = facts.iter().map(|f| f.id).collect();
    assert!(ids.contains(&pin), "пин всегда в контексте");
    assert!(ids.contains(&rel), "релевантный факт подмешан");
    assert!(!ids.contains(&other), "нерелевантный (k=1) не попал");

    // used_at обновился у подмешанных.
    let after = list(db.reader()).await.unwrap();
    assert!(after.iter().find(|f| f.id == pin).unwrap().used_at > 0);
    assert!(after.iter().find(|f| f.id == rel).unwrap().used_at > 0);
}

/// AC-MEM-4: пустой индекс/query → только пины (поиск не зовётся).
#[tokio::test]
async fn context_facts_only_pins_when_no_search() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("mem2.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };
    let pin = add(db.writer(), "всегда отвечай по-русски", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    set_pinned(db.writer(), pin, true).await.unwrap();
    // индекс пуст → top-k не ищется, но пин есть.
    let facts = context_facts(db.reader(), db.writer(), &vectors, &emb, "любой вопрос", 3)
        .await
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].id, pin);
}

/// MEM-6: порог близости отсекает хиты ниже `MEM_SIM_THRESHOLD` (раньше top-k добивался любыми).
/// Чистая функция — синтетические хиты с известными score, без эмбеддера.
#[test]
fn threshold_drops_low_similarity_hits() {
    use crate::vector::VectorHit;
    let hits = vec![
        VectorHit {
            chunk_id: 10,
            score: 0.92,
        }, // явно релевантный
        VectorHit {
            chunk_id: 11,
            score: MEM_SIM_THRESHOLD,
        }, // ровно на пороге — проходит (≥)
        VectorHit {
            chunk_id: 12,
            score: 0.10,
        }, // шум — отсекается
    ];
    let ids = ids_above_threshold(hits, MEM_SIM_THRESHOLD);
    assert_eq!(
        ids,
        vec![10, 11],
        "ниже порога отсечён, порядок ранга сохранён"
    );

    // Все ниже порога → пусто (контекст НЕ добивается нерелевантными — главный смысл MEM-6).
    let noise = vec![
        VectorHit {
            chunk_id: 1,
            score: 0.05,
        },
        VectorHit {
            chunk_id: 2,
            score: 0.20,
        },
    ];
    assert!(ids_above_threshold(noise, MEM_SIM_THRESHOLD).is_empty());
}

/// AC-MEM-9: clear вычищает все факты (смена vault).
#[tokio::test]
async fn clear_wipes_all() {
    let (_d, db) = open().await;
    add(db.writer(), "факт 1", SOURCE_EXPLICIT).await.unwrap();
    add(db.writer(), "факт 2", SOURCE_EXPLICIT).await.unwrap();
    clear(db.writer()).await.unwrap();
    assert_eq!(count(db.reader()).await.unwrap(), 0);
    assert!(list(db.reader()).await.unwrap().is_empty());
}
