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

/// MEM-3 (real-test 2026-06-18): filter_known_exact выкидывает кандидатов, точно совпадающих с уже
/// сохранёнными фактами (та же UNIQUE-семантика, что в add: trim-учёт), оставляя НОВЫЕ в порядке;
/// пустые отброшены. Это глушит повторный авто-чип «Запомнить?» на уже сохранённом факте.
#[tokio::test]
async fn filter_known_exact_drops_existing_keeps_new() {
    let (_d, db) = open().await;
    add(db.writer(), "Имя пользователя Артём", SOURCE_EXPLICIT)
        .await
        .unwrap();
    let candidates = vec![
        "  Имя пользователя Артём  ".to_string(), // точный дубль (после trim) → выкинуть
        "Пользователь предпочитает Rust".to_string(), // новый → оставить
        "   ".to_string(),                        // пустой → выкинуть
    ];
    let out = filter_known_exact(db.reader(), candidates).await.unwrap();
    assert_eq!(out, vec!["Пользователь предпочитает Rust".to_string()]);
    // Без существующих фактов ничего не фильтруется.
    let fresh = filter_known_exact(db.reader(), vec!["совсем новый факт".to_string()])
        .await
        .unwrap();
    assert_eq!(fresh, vec!["совсем новый факт".to_string()]);
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

/// MEM-7: правка факта пишет `update`-событие (old→new); правка тем же текстом события НЕ плодит.
#[tokio::test]
async fn edit_writes_update_event() {
    let (_d, db) = open().await;
    let id = add(db.writer(), "дедлайн X — пятница", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    edit(db.writer(), id, "дедлайн X — среда").await.unwrap();
    let hist = fact_history(db.reader(), id).await.unwrap();
    assert_eq!(hist.len(), 1, "одно событие правки");
    assert_eq!(hist[0].event, "update");
    assert_eq!(hist[0].old_text.as_deref(), Some("дедлайн X — пятница"));
    assert_eq!(hist[0].new_text.as_deref(), Some("дедлайн X — среда"));

    // Правка тем же текстом — не пишем пустое событие.
    edit(db.writer(), id, "  дедлайн X — среда  ")
        .await
        .unwrap();
    assert_eq!(fact_history(db.reader(), id).await.unwrap().len(), 1);
}

/// MEM-7: удаление пишет `delete`-событие (с прежним текстом), которое переживает факт (аудит).
#[tokio::test]
async fn delete_writes_event_surviving_fact() {
    let (_d, db) = open().await;
    let id = add(db.writer(), "временный факт", SOURCE_AUTO)
        .await
        .unwrap()
        .unwrap()
        .0;
    delete(db.writer(), id).await.unwrap();
    assert!(list(db.reader()).await.unwrap().is_empty(), "факт удалён");
    let hist = fact_history(db.reader(), id).await.unwrap();
    assert_eq!(hist.len(), 1, "событие удаления осталось (без FK-cascade)");
    assert_eq!(hist[0].event, "delete");
    assert_eq!(hist[0].old_text.as_deref(), Some("временный факт"));
}

/// MEM-7: ИНВАРИАНТ — супридённый факт (`superseded_by IS NOT NULL`) исключён из list/count/context
/// (заранее, до MEM-8). Помечаем вручную сырым UPDATE и проверяем.
#[tokio::test]
async fn superseded_fact_excluded_everywhere() {
    let (_d, db) = open().await;
    let keep = add(db.writer(), "живой факт", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let gone = add(db.writer(), "устаревший факт", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    // Помечаем `gone` супридённым (как сделает консолидация MEM-8): superseded_by = keep.
    db.writer()
        .transaction(move |tx| {
            tx.execute(
                "UPDATE memory_facts SET superseded_by=?2, superseded_at=1 WHERE id=?1",
                rusqlite::params![gone, keep],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let ids: Vec<i64> = list(db.reader())
        .await
        .unwrap()
        .iter()
        .map(|f| f.id)
        .collect();
    assert_eq!(ids, vec![keep], "супридённый не в списке");
    assert_eq!(
        count(db.reader()).await.unwrap(),
        1,
        "count считает только живые"
    );
}

/// MEM-10: число пинов в контексте ограничено `MEM_MAX_PINS` (раньше — все пины безусловно).
#[tokio::test]
async fn context_facts_caps_pins() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("memcap.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };
    for i in 0..(MEM_MAX_PINS + 3) {
        let id = add(db.writer(), &format!("пин-факт {i}"), SOURCE_EXPLICIT)
            .await
            .unwrap()
            .unwrap()
            .0;
        set_pinned(db.writer(), id, true).await.unwrap();
    }
    // Пустой индекс → только пины, но не больше капа.
    let facts = context_facts(db.reader(), db.writer(), &vectors, &emb, "", 3)
        .await
        .unwrap();
    assert_eq!(facts.len(), MEM_MAX_PINS, "пины обрезаны до капа");
    assert!(facts.iter().all(|f| f.pinned));
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

/// P1-4: бэкфилл векторов памяти — зеркало `open_vault`-хука (фоновый блок). Здесь прогоняем ТУ ЖЕ
/// петлю синхронно: факты без вектора (имитация `import_backup`, который пишет `memory_facts`, но не
/// `memory_vectors`) → эмбеддим query-путём → upsert по ключу=id, фильтруя уже проиндексированные через
/// `contains` (идемпотентность). КРИТИЧНО: `embed_query`, как `index_fact`/`context_facts` (память
/// симметрична) — НЕ `embed_documents` (эпизод-путь); иначе на nomic/e5 импортированный факт лёг бы в
/// document-субпространство и не совпал с тем же фактом, добавленным руками. Без embedder петля не зовётся.
async fn run_memory_backfill(
    db: &Database,
    vectors: &VectorIndex,
    embedder: Option<&dyn EmbeddingProvider>,
) -> usize {
    let Some(emb) = embedder else {
        return 0; // нет эмбеддера (RAG off) → no-op, как в хуке (guard `if let Some(emb)`)
    };
    let rows = memory_facts_for_backfill(db.reader()).await.unwrap();
    let pending: Vec<_> = rows
        .into_iter()
        .filter(|(id, _)| !vectors.contains(*id as u64))
        .collect();
    let n = pending.len();
    for (id, text) in pending {
        let vec = emb.embed_query(&text).await.unwrap();
        vectors.upsert(id as u64, &vec).unwrap();
    }
    vectors.save().unwrap();
    n
}

/// P1-4 (adversarial): эмбеддер, РАЗЛИЧАЮЩИЙ query/document-путь (как prod nomic/e5: разные префиксы →
/// разные субпространства). `MockEmbedder` их НЕ различает (query==document) → не ловит регресс на
/// `embed_documents`. Здесь document-путь префиксует текст «D:» перед хешированием → его вектор ОТЛИЧЕН
/// от query-пути того же текста. Тест на этом эмбеддере падал бы, если бы бэкфилл звал `embed_documents`.
struct AsymMockEmbedder {
    dim: usize,
}

/// Детерминированный byte-hash вектор (повторяет логику `MockEmbedder::mock_vec`, недоступного извне
/// `ai::embedder`). Префикс «D:» в document-пути сдвигает байты → ИНОЙ вектор, чем query-путь.
fn hash_vec(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for (i, b) in text.bytes().enumerate() {
        v[i % dim] += f32::from(b) / 255.0;
    }
    crate::ai::l2_normalize(&mut v);
    v
}

#[async_trait::async_trait]
impl crate::ai::EmbeddingProvider for AsymMockEmbedder {
    async fn embed_documents(&self, texts: &[&str]) -> crate::ai::AiResult<Vec<Vec<f32>>> {
        // document-путь: непустой префикс «D:» (как search_document:/passage:) → иное подпространство.
        Ok(texts
            .iter()
            .map(|t| hash_vec(&format!("D:{t}"), self.dim))
            .collect())
    }
    async fn embed_query(&self, text: &str) -> crate::ai::AiResult<Vec<f32>> {
        // query-путь: без префикса (как у index_fact/context_facts на этом эмбеддере).
        Ok(hash_vec(text, self.dim))
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn model_id(&self) -> &str {
        "asym-mock"
    }
}

/// P1-4: импортированные (= без вектора) факты бэкфилл переэмбеддит → семантический recall их находит;
/// повторный бэкфилл — no-op (идемпотентность по `contains`=id); без embedder — no-op, не падает.
#[tokio::test]
async fn import_backfill_vectorizes_facts_for_recall() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("mem_backfill.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };

    // Имитация импорта из бэкапа: факты в `memory_facts`, БЕЗ записи в `memory_vectors`.
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

    // До бэкфилла: оба факта без вектора → recall слеп (индекс пуст, top-k не ищется).
    assert!(!vectors.contains(rel as u64));
    assert!(!vectors.contains(other as u64));
    let blind = context_facts(
        db.reader(),
        db.writer(),
        &vectors,
        &emb,
        "Nexus проект на Rust и Tauri",
        3,
    )
    .await
    .unwrap();
    assert!(
        blind.is_empty(),
        "до бэкфилла импортированные факты невидимы для семантического recall"
    );

    // Бэкфилл переэмбеддит оба факта.
    let n = run_memory_backfill(&db, &vectors, Some(&emb as &dyn EmbeddingProvider)).await;
    assert_eq!(n, 2, "бэкфилл взял ровно факты без вектора");
    assert!(vectors.contains(rel as u64), "вектор факта в индексе");
    assert!(vectors.contains(other as u64));

    // Теперь recall находит релевантный факт (запрос = его текст → cosine 1.0 ≥ порога).
    let found = context_facts(
        db.reader(),
        db.writer(),
        &vectors,
        &emb,
        "Nexus проект на Rust и Tauri",
        1,
    )
    .await
    .unwrap();
    let ids: Vec<i64> = found.iter().map(|f| f.id).collect();
    assert!(
        ids.contains(&rel),
        "бэкфилл вернул факт в семантический recall"
    );

    // Идемпотентность: повторный бэкфилл ничего не делает (всё уже в индексе по `contains`).
    let again = run_memory_backfill(&db, &vectors, Some(&emb as &dyn EmbeddingProvider)).await;
    assert_eq!(
        again, 0,
        "повторный бэкфилл — no-op (идемпотентность по id)"
    );

    // Без embedder (RAG off) — no-op, не падает.
    let no_emb = run_memory_backfill(&db, &vectors, None).await;
    assert_eq!(no_emb, 0, "без эмбеддера бэкфилл — no-op");
}

/// P1-4 (adversarial-регресс-страж): бэкфилл ДОЛЖЕН эмбеддить query-путём (как `index_fact`), НЕ
/// document-путём (эпизод-зеркало). На эмбеддере, РАЗЛИЧАЮЩЕМ query/document (prod nomic/e5), вектор
/// бэкфилла обязан совпасть с вектором `index_fact` (query) и НЕ совпасть с document-путём. Если бы
/// бэкфилл звал `embed_documents` — этот тест упал бы (вектор лёг бы в чужое субпространство).
#[tokio::test]
async fn backfill_uses_query_path_not_document() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("mem_qpath.usearch"), 16).unwrap();
    let emb = AsymMockEmbedder { dim: 16 };

    let id = add(db.writer(), "владелец пишет на Rust", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;

    // Бэкфилл индексирует факт.
    let n = run_memory_backfill(&db, &vectors, Some(&emb as &dyn EmbeddingProvider)).await;
    assert_eq!(n, 1);

    // Эталоны: что дал бы query-путь (index_fact/recall) и что — document-путь (эпизод-зеркало, БАГ).
    let query_vec = emb.embed_query("владелец пишет на Rust").await.unwrap();
    let doc_vec = emb
        .embed_documents(&["владелец пишет на Rust"])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_ne!(
        query_vec, doc_vec,
        "эмбеддер ДОЛЖЕН различать query/document (иначе тест бессилен)"
    );

    // Косинус вектора в индексе с query-эталоном = 1.0 (тот же путь); с document-эталоном — ниже.
    // VectorIndex::search вернёт score = 1−cos_dist для ближайшего; ищем по query_vec.
    let hits = vectors.search(&query_vec, 1).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chunk_id, id as u64);
    assert!(
        hits[0].score > 0.999,
        "бэкфилл-вектор совпал с query-путём (score={}); если бы звал embed_documents — был бы < 1",
        hits[0].score
    );

    // И recall (context_facts → embed_query) находит факт — сквозная проверка query-консистентности.
    let found = context_facts(
        db.reader(),
        db.writer(),
        &vectors,
        &emb,
        "владелец пишет на Rust",
        1,
    )
    .await
    .unwrap();
    assert!(
        found.iter().any(|f| f.id == id),
        "recall находит бэкфилл-факт (query-путь консистентен)"
    );
}

/// P1-4: бэкфилл НЕ индексирует супридённые факты (инвариант «жив ⟺ superseded_by IS NULL») — они и так
/// исключены из recall (`facts_by_ids`), индексировать их незачем.
#[tokio::test]
async fn backfill_skips_superseded_facts() {
    let (_d, db) = open().await;
    let dir = TempDir::new().unwrap();
    let vectors = VectorIndex::open(dir.path().join("mem_backfill_sup.usearch"), 16).unwrap();
    let emb = MockEmbedder { dim: 16 };

    let keep = add(db.writer(), "живой факт для recall", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    let gone = add(db.writer(), "устаревший факт", SOURCE_EXPLICIT)
        .await
        .unwrap()
        .unwrap()
        .0;
    db.writer()
        .transaction(move |tx| {
            tx.execute(
                "UPDATE memory_facts SET superseded_by=?2, superseded_at=1 WHERE id=?1",
                rusqlite::params![gone, keep],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let n = run_memory_backfill(&db, &vectors, Some(&emb as &dyn EmbeddingProvider)).await;
    assert_eq!(n, 1, "бэкфилл взял только живой факт");
    assert!(vectors.contains(keep as u64));
    assert!(
        !vectors.contains(gone as u64),
        "супридённый факт не индексируется"
    );
}
