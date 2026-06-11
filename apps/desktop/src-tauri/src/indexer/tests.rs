//! Тесты индексатора (вынесены из mod.rs при декомпозиции #28). Дочерний модуль `indexer::tests`
//! видит приватные элементы родителя через `use super::*` (Indexer + helpers + подмодули).

use super::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

async fn open(root: &Path) -> Database {
    Database::open(root.join(".nexus/nexus.db")).await.unwrap()
}

async fn file_id(db: &Database, path: &str) -> i64 {
    let path = path.to_string();
    db.reader()
        .query(move |c| c.query_row("SELECT id FROM files WHERE path=?1", [path], |r| r.get(0)))
        .await
        .unwrap()
}

/// Источники беклинков файла `target_id` (пути), отсортированы.
async fn backlink_sources(db: &Database, target_id: i64) -> Vec<String> {
    db.reader()
        .query(move |c| {
            let mut stmt = c.prepare(
                "SELECT f.path FROM links l JOIN files f ON f.id=l.source_id \
                     WHERE l.target_id=?1 ORDER BY f.path",
            )?;
            let rows = stmt
                .query_map([target_id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
}

/// Все теги, привязанные к файлам (отсортированы).
async fn read_tags(db: &Database) -> Vec<String> {
    db.reader()
        .query(|c| {
            let mut s = c.prepare(
                "SELECT t.name FROM tags t JOIN file_tags ft ON ft.tag_id=t.id ORDER BY t.name",
            )?;
            let v = s
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .await
        .unwrap()
}

/// Алиасы файла (отсортированы).
async fn read_aliases(db: &Database, file_id: i64) -> Vec<String> {
    db.reader()
        .query(move |c| {
            let mut s = c.prepare("SELECT alias FROM aliases WHERE file_id=?1 ORDER BY alias")?;
            let v = s
                .query_map([file_id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .await
        .unwrap()
}

/// Поля frontmatter файла как `(key, value)`, отсортированы по ключу.
async fn read_fields(db: &Database, file_id: i64) -> Vec<(String, String)> {
    db.reader()
        .query(move |c| {
            let mut s = c.prepare(
                "SELECT key, value FROM frontmatter_fields WHERE file_id=?1 ORDER BY key",
            )?;
            let v = s
                .query_map([file_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .await
        .unwrap()
}

/// V4.1: `[[Алиас]]` резолвится в файл, объявивший алиас в frontmatter (forward и backward),
/// таблица `aliases` заполняется.
#[tokio::test]
async fn aliases_resolve_links_and_populate_table() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("Target.md"),
        "---\naliases: [MyAlias, Second]\n---\n# Target\n",
    )
    .unwrap();
    fs::write(root.join("Fwd.md"), "see [[MyAlias]]\n").unwrap();
    fs::write(root.join("Bwd.md"), "see [[Second]]\n").unwrap();

    let db = open(&root).await;
    let idx = Indexer::new(&db, root.clone());

    // Backward: Bwd индексируется ДО Target (ссылка висячая) → резолв при индексации Target по алиасу.
    idx.index_file("Bwd.md").await.unwrap();
    idx.index_file("Target.md").await.unwrap();
    // Forward: Fwd индексируется ПОСЛЕ Target → резолв алиаса при вставке ссылки.
    idx.index_file("Fwd.md").await.unwrap();

    let target_id = file_id(&db, "Target.md").await;
    let mut bl = backlink_sources(&db, target_id).await;
    bl.sort();
    assert_eq!(
        bl,
        vec!["Bwd.md".to_string(), "Fwd.md".to_string()],
        "[[Алиас]] резолвится и forward, и backward"
    );
    assert_eq!(
        read_aliases(&db, target_id).await,
        vec!["MyAlias".to_string(), "Second".to_string()]
    );
}

/// AC-Б9-1: atomic-save (перезапись того же пути) сохраняет file_id, беклинки целы.
#[tokio::test]
async fn atomic_save_preserves_file_id_and_backlinks() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(root.join("A.md"), "# A\n\nlink to [[B]]\n").unwrap();
    fs::write(root.join("B.md"), "# B\n").unwrap();

    let db = open(&root).await;
    let idx = Indexer::new(&db, root.clone());
    idx.index_file("B.md").await.unwrap();
    idx.index_file("A.md").await.unwrap();

    let b_id = file_id(&db, "B.md").await;
    assert_eq!(backlink_sources(&db, b_id).await, vec!["A.md"]);

    // atomic-save B.md: тот же путь, новое содержимое.
    fs::write(root.join("B.md"), "# B\n\nmore text\n").unwrap();
    idx.index_file("B.md").await.unwrap();

    assert_eq!(
        file_id(&db, "B.md").await,
        b_id,
        "file_id должен сохраниться"
    );
    assert_eq!(
        backlink_sources(&db, b_id).await,
        vec!["A.md"],
        "беклинки B не должны пострадать"
    );
}

/// AC-Б9 (V2.2): rename/move сохраняет `file_id` → беклинки целы. `[[Old]]` остаётся
/// зарезолвленной по сохранённому id, а ранее висячая `[[New]]` до-резолвится в этот файл.
#[tokio::test]
async fn rename_preserves_file_id_and_backlinks() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(root.join("Old.md"), "# Old\n").unwrap();
    fs::write(root.join("Ref.md"), "see [[Old]]\n").unwrap();
    fs::write(root.join("Fwd.md"), "see [[New]]\n").unwrap();

    let db = open(&root).await;
    let idx = Indexer::new(&db, root.clone());
    idx.index_file("Old.md").await.unwrap();
    idx.index_file("Ref.md").await.unwrap(); // [[Old]] → Old.md
    idx.index_file("Fwd.md").await.unwrap(); // [[New]] висячая (New.md ещё нет)

    let old_id = file_id(&db, "Old.md").await;
    assert_eq!(
        backlink_sources(&db, old_id).await,
        vec!["Ref.md"],
        "до rename зарезолвлена только [[Old]]"
    );

    // Переименование Old.md → New.md (как watcher после move на ФС).
    fs::rename(root.join("Old.md"), root.join("New.md")).unwrap();
    idx.rename_file("Old.md", "New.md").await.unwrap();

    assert_eq!(
        file_id(&db, "New.md").await,
        old_id,
        "file_id сохраняется под новым путём"
    );
    let mut bl = backlink_sources(&db, old_id).await;
    bl.sort();
    assert_eq!(
        bl,
        vec!["Fwd.md".to_string(), "Ref.md".to_string()],
        "[[Old]] цела (по id), [[New]] до-резолвилась"
    );

    // Старого пути в живых не осталось.
    let old_live: Option<i64> = db
        .reader()
        .query(|c| {
            c.query_row(
                "SELECT id FROM files WHERE path='Old.md' AND is_deleted=0",
                [],
                |r| r.get(0),
            )
            .optional()
        })
        .await
        .unwrap();
    assert!(old_live.is_none(), "старый путь не должен оставаться живым");
}

/// Обратный резолв: ссылка, чья цель проиндексирована позже, до-резолвится.
#[tokio::test]
async fn back_resolves_links_indexed_out_of_order() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(root.join("A.md"), "[[B]]\n").unwrap();
    fs::write(root.join("B.md"), "# B\n").unwrap();

    let db = open(&root).await;
    let idx = Indexer::new(&db, root.clone());
    idx.index_file("A.md").await.unwrap(); // B ещё не в БД → ссылка висячая
    idx.index_file("B.md").await.unwrap(); // обратный резолв привяжет ссылку A→B

    let b_id = file_id(&db, "B.md").await;
    assert_eq!(backlink_sources(&db, b_id).await, vec!["A.md"]);
}

/// Индексация наполняет теги; повторная индексация заменяет их.
#[tokio::test]
async fn indexes_and_replaces_tags() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(root.join("N.md"), "body #project #area\n").unwrap();

    let db = open(&root).await;
    let idx = Indexer::new(&db, root.clone());
    idx.index_file("N.md").await.unwrap();

    assert_eq!(
        read_tags(&db).await,
        vec!["area".to_string(), "project".to_string()]
    );

    fs::write(root.join("N.md"), "body #area only\n").unwrap();
    idx.index_file("N.md").await.unwrap();
    assert_eq!(read_tags(&db).await, vec!["area".to_string()]);
}

/// #35 хвост: frontmatter `tags:` индексируются в file_tags наравне с инлайн-тегами тела
/// (раньше `tags: [goal]` не давал file_tag — маркер цели работал только инлайном).
#[tokio::test]
async fn indexes_frontmatter_tags_into_file_tags() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("G.md"),
        "---\ntags: [goal, Project]\n---\nтело с #inline\n",
    )
    .unwrap();

    let db = open(&root).await;
    let idx = Indexer::new(&db, root.clone());
    idx.index_file("G.md").await.unwrap();

    assert_eq!(
        read_tags(&db).await,
        vec![
            "goal".to_string(),
            "inline".to_string(),
            "project".to_string()
        ]
    );
}

/// typed-frontmatter: плоские поля индексируются в `frontmatter_fields` и заменяются при реиндексе.
#[tokio::test]
async fn indexes_and_replaces_frontmatter_fields() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("Goal.md"),
        "---\nprogress: 0.3\ndue: 2026-02-01\naliases: [G]\n---\nbody\n",
    )
    .unwrap();

    let db = open(&root).await;
    let idx = Indexer::new(&db, root.clone());
    idx.index_file("Goal.md").await.unwrap();
    let id = file_id(&db, "Goal.md").await;

    // Плоские скаляры записаны; список aliases в frontmatter_fields НЕ попал (у него своя таблица).
    assert_eq!(
        read_fields(&db, id).await,
        vec![
            ("due".to_string(), "2026-02-01".to_string()),
            ("progress".to_string(), "0.3".to_string()),
        ]
    );

    // Реиндекс с другими полями → полная замена (старое `due` ушло).
    fs::write(root.join("Goal.md"), "---\nprogress: 1.0\n---\nbody\n").unwrap();
    idx.index_file("Goal.md").await.unwrap();
    assert_eq!(
        read_fields(&db, id).await,
        vec![("progress".to_string(), "1.0".to_string())]
    );
    assert_eq!(
        file_id(&db, "Goal.md").await,
        id,
        "file_id стабилен (UPSERT по пути)"
    );
}

// ── RAG (Ф1-5): чанки + эмбеддинги + usearch ──────────────────────────────────────────────

use crate::ai::MockEmbedder;

/// Индексатор с RAG поверх детерминированного мок-эмбеддера и собственного usearch-файла.
fn rag_indexer(db: &Database, root: &Path, dim: usize, force: bool) -> (Indexer, Arc<VectorIndex>) {
    let path = root.join(".nexus").join("vectors.usearch");
    let vectors = Arc::new(VectorIndex::open(path, dim).unwrap());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim });
    let idx = Indexer::with_rag(db, root.to_path_buf(), embedder, vectors.clone(), force);
    (idx, vectors)
}

async fn chunk_count(db: &Database) -> i64 {
    db.reader()
        .query(|c| c.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0)))
        .await
        .unwrap()
}

async fn fts_hits(db: &Database, term: &str) -> i64 {
    let term = term.to_string();
    db.reader()
        .query(move |c| {
            c.query_row(
                "SELECT count(*) FROM fts_chunks WHERE fts_chunks MATCH ?1",
                [term],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
}

/// AC-Б4-1 / AC-Б8-1: индексация пишет чанки, наполняет FTS и кладёт по вектору на чанк.
#[tokio::test]
async fn rag_index_writes_chunks_fts_and_vectors() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("Note.md"),
        "# Heading\n\nalpha beta gamma vector search body text here\n",
    )
    .unwrap();

    let db = open(&root).await;
    let (idx, vectors) = rag_indexer(&db, &root, 16, false);
    idx.index_file("Note.md").await.unwrap();

    let n = chunk_count(&db).await;
    assert!(n >= 1, "должен появиться хотя бы один чанк");
    assert_eq!(vectors.len(), n as usize, "по вектору на чанк (AC-Б4-1)");
    assert_eq!(fts_hits(&db, "vector").await, 1, "FTS находит тело чанка");
}

/// AC-Б9 (V2.2): rename сохраняет чанки и векторы под тем же `file_id` (не пересоздаёт) —
/// чистый rename проходит через ранний выход `index_file` (mtime/size не изменились).
#[tokio::test]
async fn rename_preserves_chunks_and_vectors() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("Old.md"),
        "# Heading\n\nalpha beta gamma vector search body text here\n",
    )
    .unwrap();

    let db = open(&root).await;
    let (idx, vectors) = rag_indexer(&db, &root, 16, false);
    idx.index_file("Old.md").await.unwrap();
    let before = chunk_count(&db).await;
    assert!(before >= 1, "должен появиться хотя бы один чанк");
    let old_id = file_id(&db, "Old.md").await;

    fs::rename(root.join("Old.md"), root.join("New.md")).unwrap();
    idx.rename_file("Old.md", "New.md").await.unwrap();

    assert_eq!(file_id(&db, "New.md").await, old_id, "file_id сохранён");
    assert_eq!(chunk_count(&db).await, before, "число чанков не изменилось");
    assert_eq!(
        vectors.len(),
        before as usize,
        "векторы целы (по одному на чанк)"
    );
    assert_eq!(
        fts_hits(&db, "vector").await,
        1,
        "FTS по-прежнему находит чанк переименованного файла"
    );
}

/// AC-Б4-2 (интеграция): реиндексация заменяет чанки и векторы без осиротевших — число
/// векторов = числу чанков, старый текст уходит из FTS, новый появляется.
#[tokio::test]
async fn reindex_replaces_chunks_and_vectors_without_orphans() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("Note.md"),
        "# H\n\nalpha vector search body words\n",
    )
    .unwrap();

    let db = open(&root).await;
    let (idx, vectors) = rag_indexer(&db, &root, 16, false);
    idx.index_file("Note.md").await.unwrap();
    assert_eq!(fts_hits(&db, "vector").await, 1);

    // Иное содержимое (другой размер → mtime-шорткат не сработает) → полная замена.
    fs::write(root.join("Note.md"), "# H\n\ndelta epsilon zeta\n").unwrap();
    idx.index_file("Note.md").await.unwrap();

    assert_eq!(
        vectors.len(),
        chunk_count(&db).await as usize,
        "нет осиротевших векторов после реиндексации (AC-Б4-2)"
    );
    assert_eq!(fts_hits(&db, "vector").await, 0, "старый текст ушёл из FTS");
    assert_eq!(fts_hits(&db, "delta").await, 1, "новый текст попал в FTS");
}

/// AC-Б8-2 (интеграция): удаление файла чистит и чанки (+FTS), и векторы usearch.
#[tokio::test]
async fn remove_file_purges_chunks_and_vectors() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(root.join("Note.md"), "# H\n\nalpha vector beta gamma\n").unwrap();

    let db = open(&root).await;
    let (idx, vectors) = rag_indexer(&db, &root, 16, false);
    idx.index_file("Note.md").await.unwrap();
    assert!(!vectors.is_empty());

    idx.remove_file("Note.md").await.unwrap();
    assert_eq!(chunk_count(&db).await, 0, "чанки удалены");
    assert_eq!(vectors.len(), 0, "векторы удалены из usearch");
    assert_eq!(fts_hits(&db, "vector").await, 0, "FTS чист");
}

/// §6.5 (AC-Б5-2): `force` переиндексирует НЕизменённый файл (mtime/size те же) — так после
/// смены модели чанки и векторы перестраиваются, хотя файлы на диске не трогали.
#[tokio::test]
async fn force_reindex_rebuilds_unchanged_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("Note.md"),
        "# H\n\nalpha vector beta gamma delta\n",
    )
    .unwrap();

    let db = open(&root).await;
    let (idx, _v1) = rag_indexer(&db, &root, 16, false);
    idx.index_file("Note.md").await.unwrap();
    let n = chunk_count(&db).await;
    assert!(n >= 1);

    // Имитируем смену модели: чанки очищены (как делает reconcile), usearch — новый файл.
    db.writer()
        .call(|c| c.execute("DELETE FROM chunks", []).map(|_| ()))
        .await
        .unwrap();
    assert_eq!(chunk_count(&db).await, 0);

    let vectors2 =
        Arc::new(VectorIndex::open(root.join(".nexus").join("vectors2.usearch"), 16).unwrap());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
    let idx2 = Indexer::with_rag(&db, root.clone(), embedder, vectors2.clone(), true);
    idx2.index_file("Note.md").await.unwrap(); // файл НЕ менялся, но force обходит шорткат

    assert_eq!(
        chunk_count(&db).await,
        n,
        "force переиндексировал несмотря на mtime-шорткат (§6.5)"
    );
    assert_eq!(vectors2.len(), n as usize);
}

/// §5.1 crash-reconcile: потерянный вектор (chunks в БД есть, вектора в usearch нет) дочиняется.
#[tokio::test]
async fn reconcile_restores_lost_vectors() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("Note.md"),
        "# H\n\nalpha vector beta gamma delta\n",
    )
    .unwrap();

    let db = open(&root).await;
    let (idx, vectors) = rag_indexer(&db, &root, 16, false);
    idx.index_file("Note.md").await.unwrap();
    let n = vectors.len();
    assert!(n >= 1);

    // Имитируем крах: вектор пропал из usearch, но чанк в БД остался.
    let lost: i64 = db
        .reader()
        .query(|c| c.query_row("SELECT id FROM chunks LIMIT 1", [], |r| r.get(0)))
        .await
        .unwrap();
    vectors.remove(lost as u64).unwrap();
    assert!(!vectors.contains(lost as u64));
    assert_eq!(vectors.len(), n - 1);

    // reconcile переэмбеддит и возвращает потерянный вектор.
    idx.reconcile_vectors().await.unwrap();
    assert!(
        vectors.contains(lost as u64),
        "reconcile вернул потерянный вектор"
    );
    assert_eq!(vectors.len(), n);
}

/// Живой end-to-end против nomic на :8081 (`cargo test -- --ignored`): индексируем два файла,
/// семантический запрос про кошку находит чанк именно из cat.md (а не из физики).
#[tokio::test]
#[ignore = "нужен embedding-сервер (NEXUS_EMBED_URL, default 192.168.0.31:8083)"]
async fn live_rag_index_and_semantic_search() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("cat.md"),
        "# Кошка\n\nКошка сидит на тёплом коврике у окна и довольно мурлычет.\n",
    )
    .unwrap();
    fs::write(
        root.join("physics.md"),
        "# Физика\n\nКвантовая хромодинамика описывает сильное взаимодействие кварков.\n",
    )
    .unwrap();

    let db = open(&root).await;
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::ai::live_test_embedder());
    let vectors = Arc::new(
        VectorIndex::open(
            root.join(".nexus").join("vectors.usearch"),
            crate::ai::LIVE_EMBED_DIM,
        )
        .unwrap(),
    );
    let idx = Indexer::with_rag(&db, root.clone(), embedder.clone(), vectors.clone(), true);
    idx.index_file("cat.md").await.unwrap();
    idx.index_file("physics.md").await.unwrap();
    assert!(vectors.len() >= 2, "оба файла дали векторы");

    let q = embedder.embed_query("где находится кошка?").await.unwrap();
    let hits = vectors.search(&q, 1).unwrap();
    let top = hits[0].chunk_id as i64;
    let path: String = db
        .reader()
        .query(move |c| {
            c.query_row(
                "SELECT f.path FROM chunks ch JOIN files f ON f.id=ch.file_id WHERE ch.id=?1",
                [top],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        path, "cat.md",
        "ближайший к запросу про кошку чанк — из cat.md"
    );
}

/// Фикс «вечных воркеров» (аудит 2026-06-10): петля событий обрабатывает Upsert и ШТАТНО
/// ЗАВЕРШАЕТСЯ, когда sender канала дропнут (= VaultWatcher дропнут из VaultContext при
/// повторном open_vault). Раньше watcher жил внутри задачи → петля не завершалась никогда.
#[tokio::test]
async fn event_loop_indexes_and_stops_when_sender_dropped() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join(".nexus")).unwrap();
    fs::write(root.join("note.md"), "# Заметка\n\nтекст").unwrap();
    let db = open(&root).await;
    let indexer = Indexer::new(&db, root.clone());

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<crate::watcher::VaultEvent>();
    let notified = Arc::new(AtomicUsize::new(0));
    let n2 = notified.clone();
    let handle = tokio::spawn(events::event_loop(indexer, rx, move || {
        n2.fetch_add(1, Ordering::SeqCst);
    }));

    // Событие обрабатывается (файл попадает в индекс)...
    fs::write(root.join("new.md"), "# Новая\n\nещё текст").unwrap();
    tx.send(crate::watcher::VaultEvent::Upsert(root.join("new.md")))
        .unwrap();
    // ...дроп sender'а (= дроп watcher'а из VaultContext) штатно завершает петлю.
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("петля обязана завершиться после дропа sender (не вечная)")
        .expect("петля завершилась без паники");

    assert!(
        file_id(&db, "new.md").await > 0,
        "Upsert до дропа обработан"
    );
    assert!(
        notified.load(Ordering::SeqCst) >= 2,
        "нотификации: начальный скан + событие"
    );
}

/// Срез «Переиндексировать» (#37): `VaultEvent::Rescan` в петле — полный повторный обход.
/// Файл, созданный МИМО watcher-событий (петля о нём не знает), попадает в индекс после Rescan;
/// по завершении прилетает нотификация (фронт перечитывает вьюхи по `vault:changed`).
#[tokio::test]
async fn event_loop_rescan_picks_up_unseen_files() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join(".nexus")).unwrap();
    let db = open(&root).await;
    let indexer = Indexer::new(&db, root.clone());

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<crate::watcher::VaultEvent>();
    let notified = Arc::new(AtomicUsize::new(0));
    let n2 = notified.clone();
    let handle = tokio::spawn(events::event_loop(indexer, rx, move || {
        n2.fetch_add(1, Ordering::SeqCst);
    }));

    // Файл появляется без Upsert-события (watcher «проспал» / файл писали вне приложения)…
    fs::write(root.join("ghost.md"), "# Призрак\n\nвне watcher").unwrap();
    tx.send(crate::watcher::VaultEvent::Rescan).unwrap();
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("петля завершается после дропа sender")
        .expect("без паники");

    assert!(
        file_id(&db, "ghost.md").await > 0,
        "Rescan индексирует файл, не прошедший через события"
    );
    assert!(
        notified.load(Ordering::SeqCst) >= 2,
        "нотификации: начальный скан + rescan"
    );
}

/// Срез «прогресс индексации» (ночь 2026-06-11): хук `with_progress` зовётся на старте (0, total),
/// на финише (total, total), done монотонен и не превышает total.
#[tokio::test]
async fn scan_progress_hook_reports_start_and_finish() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    for i in 0..5 {
        fs::write(root.join(format!("n{i}.md")), format!("# n{i}\n")).unwrap();
    }

    let db = open(&root).await;
    let calls: std::sync::Arc<std::sync::Mutex<Vec<(usize, usize)>>> = Default::default();
    let sink = calls.clone();
    let idx = Indexer::new(&db, root.clone()).with_progress(move |done, total| {
        sink.lock().unwrap().push((done, total));
    });
    idx.scan_vault().await.unwrap();

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.first(), Some(&(0, 5)), "старт — (0, total)");
    assert_eq!(calls.last(), Some(&(5, 5)), "финиш — (total, total)");
    assert!(calls.iter().all(|&(d, t)| d <= t && t == 5));
    assert!(calls.windows(2).all(|w| w[0].0 <= w[1].0), "done монотонен");
}
