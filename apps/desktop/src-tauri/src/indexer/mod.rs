//! Инкрементальный индексатор (§4.2, §6.1): парсит .md и пишет `files/links/tags` через
//! write-actor. Стабильность `file_id` при atomic-save — за счёт UPSERT по `path` (AC-Б9-1).
//!
//! Ссылки резолвятся в обе стороны: прямо (исходящие ссылки файла → `target_id`) и обратно
//! (висячие ссылки, чья цель проиндексирована позже, до-резолвятся при появлении файла).
//! Chunks/embeddings (RAG) — Фаза 1; здесь только граф ссылок, теги и метаданные.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, OptionalExtension, Transaction};

use crate::db::{Database, DbError, DbResult, ReadPool, WriteActor};
use crate::parser;
use crate::watcher::{self, VaultEvent, VaultWatcher};

/// Индексатор одного vault. Дёшево клонируемые writer/reader + корень.
pub struct Indexer {
    writer: WriteActor,
    reader: ReadPool,
    root: PathBuf,
}

impl Indexer {
    pub fn new(db: &Database, root: PathBuf) -> Self {
        Self {
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            root,
        }
    }

    /// Индексирует один файл по относительному пути. Для не-.md — no-op. Пропускает
    /// неизменённые файлы по mtime+size (дешёвый шорткат — не читаем диск зря).
    pub async fn index_file(&self, rel: &str) -> DbResult<()> {
        if !rel.ends_with(".md") {
            return Ok(());
        }
        let abs = self.root.join(rel);
        let Ok(meta) = tokio::fs::metadata(&abs).await else {
            return Ok(()); // файла нет — обрабатывается как Deleted отдельно
        };
        let size = meta.len() as i64;
        let mtime = mtime_secs(&meta);

        let rel_owned = rel.to_string();
        let unchanged = self
            .reader
            .query({
                let rel = rel_owned.clone();
                move |c| {
                    c.query_row(
                        "SELECT updated_at, size_bytes FROM files WHERE path=?1 AND is_deleted=0",
                        [rel],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                    )
                    .optional()
                }
            })
            .await?;
        if let Some((u, s)) = unchanged {
            if u == mtime && s == size {
                return Ok(());
            }
        }

        let content = tokio::fs::read_to_string(&abs).await.unwrap_or_default();
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
        let parsed = tokio::task::spawn_blocking(move || parser::parse(&content))
            .await
            .map_err(|_| DbError::Unavailable)?;

        let forms = path_forms(&rel_owned);
        let now = now_secs();

        self.writer
            .transaction(move |tx| {
                let file_id: i64 = tx.query_row(
                    "INSERT INTO files \
                       (path,hash,title,created_at,updated_at,indexed_at,size_bytes,word_count,frontmatter) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) \
                     ON CONFLICT(path) DO UPDATE SET \
                       hash=excluded.hash, title=excluded.title, updated_at=excluded.updated_at, \
                       indexed_at=excluded.indexed_at, size_bytes=excluded.size_bytes, \
                       word_count=excluded.word_count, frontmatter=excluded.frontmatter, is_deleted=0 \
                     RETURNING id",
                    params![
                        rel_owned,
                        hash,
                        parsed.title,
                        mtime,
                        mtime,
                        now,
                        size,
                        parsed.word_count as i64,
                        parsed.frontmatter,
                    ],
                    |r| r.get(0),
                )?;

                // Исходящие ссылки: полная замена (DELETE + INSERT с прямым резолвом цели).
                tx.execute("DELETE FROM links WHERE source_id=?1", [file_id])?;
                for link in &parsed.links {
                    let target_id = resolve_target(tx, &link.target_raw)?;
                    tx.execute(
                        "INSERT INTO links (source_id,target_id,target_raw,link_type,context,line_number) \
                         VALUES (?1,?2,?3,?4,?5,?6)",
                        params![
                            file_id,
                            target_id,
                            link.target_raw,
                            link.link_type.as_str(),
                            link.context,
                            link.line_number as i64,
                        ],
                    )?;
                }

                // Теги: полная замена связей файла.
                tx.execute("DELETE FROM file_tags WHERE file_id=?1", [file_id])?;
                for tag in &parsed.tags {
                    tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
                    let tag_id: i64 =
                        tx.query_row("SELECT id FROM tags WHERE name=?1", [tag], |r| r.get(0))?;
                    tx.execute(
                        "INSERT OR IGNORE INTO file_tags (file_id,tag_id) VALUES (?1,?2)",
                        params![file_id, tag_id],
                    )?;
                }

                // Обратный резолв: висячие ссылки на этот файл получают target_id.
                for form in &forms {
                    tx.execute(
                        "UPDATE links SET target_id=?1 WHERE target_id IS NULL AND target_raw=?2",
                        params![file_id, form],
                    )?;
                }
                Ok(())
            })
            .await
    }

    /// Soft-delete файла: помечает удалённым, обнуляет входящие ссылки, чистит исходящие/теги.
    pub async fn remove_file(&self, rel: &str) -> DbResult<()> {
        let rel = rel.to_string();
        self.writer
            .transaction(move |tx| {
                let id: Option<i64> = tx
                    .query_row("SELECT id FROM files WHERE path=?1", [&rel], |r| r.get(0))
                    .optional()?;
                if let Some(id) = id {
                    tx.execute("UPDATE files SET is_deleted=1 WHERE id=?1", [id])?;
                    tx.execute("UPDATE links SET target_id=NULL WHERE target_id=?1", [id])?;
                    tx.execute("DELETE FROM links WHERE source_id=?1", [id])?;
                    tx.execute("DELETE FROM file_tags WHERE file_id=?1", [id])?;
                }
                Ok(())
            })
            .await
    }

    /// Начальный обход vault: индексирует все .md, затем до-резолвит висячие ссылки.
    pub async fn scan_vault(&self) -> DbResult<()> {
        let root = self.root.clone();
        let rels = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            collect_md(&root, &root, &mut out);
            out
        })
        .await
        .map_err(|_| DbError::Unavailable)?;

        let total = rels.len();
        for rel in rels {
            if let Err(e) = self.index_file(&rel).await {
                tracing::warn!(file = %rel, error = %e, "index_file failed during scan");
            }
        }
        self.writer.transaction(resolve_all_dangling).await?;
        tracing::info!(files = total, "initial vault scan complete");
        Ok(())
    }
}

/// Запускает watcher + фоновый цикл индексации для vault (вызывается из `open_vault`).
/// Watcher живёт внутри спавненной задачи; на завершении приложения останавливается.
pub fn spawn(db: &Database, root: PathBuf) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VaultEvent>();
    let watcher = match VaultWatcher::new(&root, tx) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "vault watcher init failed");
            return;
        }
    };
    let indexer = Indexer::new(db, root);
    tokio::spawn(async move {
        let _watcher = watcher; // держим watcher живым на время задачи
        if let Err(e) = indexer.scan_vault().await {
            tracing::error!(error = %e, "initial vault scan failed");
        }
        while let Some(event) = rx.recv().await {
            let result = match event {
                VaultEvent::Upsert(abs) => match rel_of(&indexer.root, &abs) {
                    Some(rel) => indexer.index_file(&rel).await,
                    None => Ok(()),
                },
                VaultEvent::Deleted(abs) => match rel_of(&indexer.root, &abs) {
                    Some(rel) => indexer.remove_file(&rel).await,
                    None => Ok(()),
                },
            };
            if let Err(e) = result {
                tracing::warn!(error = %e, "index event failed");
            }
        }
    });
}

/// Резолвит цель ссылки в `file_id` (точный путь, путь+`.md`, basename ± `.md`).
fn resolve_target(tx: &Transaction, target_raw: &str) -> rusqlite::Result<Option<i64>> {
    tx.query_row(
        "SELECT id FROM files WHERE is_deleted=0 AND ( \
           path = ?1 OR path = ?1 || '.md' \
           OR path LIKE '%/' || ?1 OR path LIKE '%/' || ?1 || '.md' \
         ) ORDER BY length(path) LIMIT 1",
        [target_raw],
        |r| r.get(0),
    )
    .optional()
}

/// До-резолвит ВСЕ висячие ссылки (после начального скана — закрывает порядок индексации).
fn resolve_all_dangling(tx: &Transaction) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE links SET target_id = ( \
           SELECT f.id FROM files f WHERE f.is_deleted=0 AND ( \
             f.path = links.target_raw OR f.path = links.target_raw || '.md' \
             OR f.path LIKE '%/' || links.target_raw OR f.path LIKE '%/' || links.target_raw || '.md' \
           ) ORDER BY length(f.path) LIMIT 1 \
         ) WHERE target_id IS NULL",
        [],
    )?;
    Ok(())
}

/// Нормализованные формы относительного пути для обратного резолва ссылок.
fn path_forms(rel: &str) -> Vec<String> {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    let mut forms = vec![
        rel.to_string(),
        rel.strip_suffix(".md").unwrap_or(rel).to_string(),
        base.to_string(),
        base.strip_suffix(".md").unwrap_or(base).to_string(),
    ];
    forms.sort();
    forms.dedup();
    forms
}

fn collect_md(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if watcher::is_ignored(&path) {
            continue;
        }
        if path.is_dir() {
            collect_md(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(rel) = rel_of(root, &path) {
                out.push(rel);
            }
        }
    }
}

fn rel_of(root: &Path, abs: &Path) -> Option<String> {
    abs.strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
}
