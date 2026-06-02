//! Команды vault: открытие хранилища и ленивый листинг каталогов.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::OptionalExtension;
use tauri::State;

use crate::ai::{
    self, ChatProvider, EmbeddingProvider, LocalConfig, OpenAiChatProvider, OpenAiEmbedder,
};
use crate::db::Database;
use crate::state::{AppState, VaultContext};
use crate::vault::{self, FileEntry, NoteRef, VaultInfo};
use crate::vector::VectorIndex;

/// Открывает vault: канонизирует папку, открывает БД в `.nexus/nexus.db`, сохраняет в state.
#[tauri::command]
pub async fn open_vault(state: State<'_, AppState>, path: String) -> Result<VaultInfo, String> {
    let root = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| format!("vault path: {e}"))?;
    if !root.is_dir() {
        return Err("vault: путь не является каталогом".into());
    }

    let db = Database::open(root.join(".nexus").join("nexus.db"))
        .await
        .map_err(|e| e.to_string())?;

    let info = VaultInfo {
        root: root.to_string_lossy().into_owned(),
        name: vault::vault_name(&root),
    };

    // RAG (Ф1-5): строим эмбеддер + векторный индекс из .nexus/local.json. Если конфига нет
    // или embedding-сервер недоступен — vault открывается без AI (local-first).
    let (vectors, embedder, indexer) = match build_rag(&root, &db).await {
        Some((embedder, vec_index, force)) => {
            let idx = crate::indexer::Indexer::with_rag(
                &db,
                root.clone(),
                embedder.clone(),
                vec_index.clone(),
                force,
            );
            (Some(vec_index), Some(embedder), idx)
        }
        None => (None, None, crate::indexer::Indexer::new(&db, root.clone())),
    };

    // Chat-провайдер (ADR-005: отдельный хост) — независимо от embedding RAG.
    let chat = build_chat(&root).await;

    // Запускаем watcher + фоновую индексацию (начальный скан + инкрементальные события).
    crate::indexer::spawn(indexer);

    *state.vault.write().await = Some(VaultContext {
        root,
        db,
        vectors,
        embedder,
        chat,
    });
    tracing::info!(vault = %info.root, "opened vault");
    Ok(info)
}

/// Строит RAG-подсистему из `.nexus/local.json`. `None` — конфига нет / нет embedding-секции /
/// сервер недоступен (RAG отключается, vault работает без AI). Делает реконсиляцию модели (§6.5).
async fn build_rag(
    root: &Path,
    db: &Database,
) -> Option<(Arc<dyn EmbeddingProvider>, Arc<VectorIndex>, bool)> {
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .ok()?;
    let cfg = LocalConfig::parse(&raw)
        .map_err(|e| tracing::warn!(error = %e, "local.json: разбор не удался — RAG отключён"))
        .ok()?;
    let emb = cfg.ai.embedding?;
    let model = emb.model.clone().unwrap_or_else(|| "embedding".to_string());

    // Размерность: из конфига или пробным эмбеддингом у сервера (§6.5 — не хардкод).
    let dim = match emb.dim {
        Some(d) => d,
        None => OpenAiEmbedder::probe_dim(&emb.url, &model)
            .await
            .map_err(|e| tracing::warn!(error = %e, "проба размерности не удалась — RAG отключён"))
            .ok()?,
    };

    let embedder = OpenAiEmbedder::new(&emb.url, &model, dim, ai::default_prefixes(&model))
        .map_err(|e| tracing::warn!(error = %e, "эмбеддер не инициализирован — RAG отключён"))
        .ok()?;

    // §6.5: смена модели/размерности инвалидирует чанки и векторы → force-переиндексация.
    let force = reconcile_embedding_model(db, root, &model, dim)
        .await
        .ok()?;

    let vectors = VectorIndex::open(root.join(".nexus").join("vectors.usearch"), dim)
        .map_err(|e| tracing::warn!(error = %e, "usearch open не удался — RAG отключён"))
        .ok()?;

    tracing::info!(model = %model, dim, force, "RAG включён");
    Some((Arc::new(embedder), Arc::new(vectors), force))
}

/// Строит chat-провайдер из `.nexus/local.json` (`ai.chat`). `None`, если секции нет или клиент
/// не инициализировался. Доступность сервера здесь НЕ проверяем — это выяснится при первом стриме.
async fn build_chat(root: &Path) -> Option<Arc<dyn ChatProvider>> {
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .ok()?;
    let chat = LocalConfig::parse(&raw).ok()?.ai.chat?;
    let model = chat.model.clone().unwrap_or_else(|| "chat".to_string());
    let provider = OpenAiChatProvider::new(&chat.url, &model, None)
        .map_err(|e| tracing::warn!(error = %e, "chat-провайдер не инициализирован"))
        .ok()?;
    tracing::info!(model = %model, "chat-провайдер включён");
    Some(Arc::new(provider))
}

/// Сверяет активную модель/размерность эмбеддера с `settings`. При расхождении на НЕпервом запуске
/// чистит `chunks` (+FTS триггерами) и файл векторов, пишет новые `settings`. Возвращает `force` —
/// нужна ли принудительная переиндексация (первое включение RAG ИЛИ смена модели §6.5).
async fn reconcile_embedding_model(
    db: &Database,
    root: &Path,
    model: &str,
    dim: usize,
) -> Result<bool, ()> {
    let prev_model = get_setting(db, "embedding.model")
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile: чтение settings — RAG отключён"))?;
    let prev_dim = get_setting(db, "embedding.dim")
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile: чтение settings — RAG отключён"))?;
    if prev_model.as_deref() == Some(model) && prev_dim.as_deref() == Some(&dim.to_string()) {
        return Ok(false); // та же модель — инкрементальная индексация, без переэмбеддизации
    }
    if prev_model.is_some() {
        // Модель сменилась → старые векторы несовместимы по семантике/размерности.
        db.writer()
            .call(|c| c.execute("DELETE FROM chunks", []).map(|_| ()))
            .await
            .map_err(|e| tracing::warn!(error = %e, "reconcile: очистка chunks — RAG отключён"))?;
        let _ = std::fs::remove_file(root.join(".nexus").join("vectors.usearch"));
        tracing::info!(from = ?prev_model, to = %model, "смена embedding-модели → переэмбеддизация vault (§6.5)");
    }
    set_setting(db, "embedding.model", model)
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile: запись settings — RAG отключён"))?;
    set_setting(db, "embedding.dim", &dim.to_string())
        .await
        .map_err(|e| tracing::warn!(error = %e, "reconcile: запись settings — RAG отключён"))?;
    Ok(true)
}

/// Читает значение из таблицы `settings` (или `None`).
async fn get_setting(db: &Database, key: &str) -> Result<Option<String>, String> {
    let key = key.to_string();
    db.reader()
        .query(move |c| {
            c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .optional()
        })
        .await
        .map_err(|e| e.to_string())
}

/// Upsert значения в таблицу `settings`.
async fn set_setting(db: &Database, key: &str, value: &str) -> Result<(), String> {
    let key = key.to_string();
    let value = value.to_string();
    db.writer()
        .call(move |c| {
            c.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![key, value],
            )
            .map(|_| ())
        })
        .await
        .map_err(|e| e.to_string())
}

/// Ленивый листинг каталога vault (`dir_path` относительный; пустая строка = корень).
#[tauri::command]
pub async fn list_dir(
    state: State<'_, AppState>,
    dir_path: String,
) -> Result<Vec<FileEntry>, String> {
    // Копируем корень и отпускаем лок: ФС-обход уводим в blocking-пул.
    let root = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.root.clone()
    };
    tokio::task::spawn_blocking(move || vault::list_dir(&root, Path::new(&dir_path)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Читает содержимое файла vault (путь относительный; анти-traversal через resolve).
#[tauri::command]
pub async fn read_file(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path(&root, Path::new(&path)).map_err(|e| e.to_string())?;
    tokio::fs::read_to_string(&abs)
        .await
        .map_err(|e| e.to_string())
}

/// Пишет содержимое файла vault (целевой путь может ещё не существовать).
#[tauri::command]
pub async fn write_file(
    state: State<'_, AppState>,
    path: String,
    content: String,
) -> Result<(), String> {
    let root = current_root(&state).await?;
    let abs =
        vault::resolve_vault_path_for_write(&root, Path::new(&path)).map_err(|e| e.to_string())?;
    tokio::fs::write(&abs, content)
        .await
        .map_err(|e| e.to_string())
}

/// Все заметки vault (path + title) — для автокомплита `[[wikilink]]` и поиска.
#[tauri::command]
pub async fn list_notes(state: State<'_, AppState>) -> Result<Vec<NoteRef>, String> {
    let reader = {
        let guard = state.vault.read().await;
        guard.as_ref().ok_or("vault не открыт")?.db.reader().clone()
    };
    reader
        .query(|c| {
            let mut stmt =
                c.prepare("SELECT path, title FROM files WHERE is_deleted=0 ORDER BY path")?;
            let notes = stmt
                .query_map([], |r| {
                    Ok(NoteRef {
                        path: r.get(0)?,
                        title: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(notes)
        })
        .await
        .map_err(|e| e.to_string())
}

/// Корень текущего открытого vault (или ошибка, если не открыт).
async fn current_root(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    let guard = state.vault.read().await;
    Ok(guard.as_ref().ok_or("vault не открыт")?.root.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn open_db(root: &Path) -> Database {
        Database::open(root.join(".nexus/nexus.db")).await.unwrap()
    }

    async fn count_chunks(db: &Database) -> i64 {
        db.reader()
            .query(|c| c.query_row("SELECT count(*) FROM chunks", [], |r| r.get(0)))
            .await
            .unwrap()
    }

    /// §6.5: первое включение RAG пишет settings и требует force; та же модель — без force.
    #[tokio::test]
    async fn reconcile_first_run_sets_settings_and_forces() {
        let dir = TempDir::new().unwrap();
        let db = open_db(dir.path()).await;

        let force = reconcile_embedding_model(&db, dir.path(), "nomic", 768)
            .await
            .unwrap();
        assert!(force, "первое включение RAG → force-переиндексация");
        assert_eq!(
            get_setting(&db, "embedding.model")
                .await
                .unwrap()
                .as_deref(),
            Some("nomic")
        );
        assert_eq!(
            get_setting(&db, "embedding.dim").await.unwrap().as_deref(),
            Some("768")
        );

        let again = reconcile_embedding_model(&db, dir.path(), "nomic", 768)
            .await
            .unwrap();
        assert!(!again, "та же модель/dim → без переэмбеддизации");
    }

    /// §6.5 (AC-Б5-2): смена модели чистит chunks (+FTS триггерами) и требует force.
    #[tokio::test]
    async fn reconcile_model_change_wipes_chunks_and_forces() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let db = open_db(root).await;
        reconcile_embedding_model(&db, root, "nomic", 768)
            .await
            .unwrap();

        // Файл + чанк (как после индексации).
        db.writer()
            .call(|c| {
                c.execute(
                    "INSERT INTO files (path,hash,created_at,updated_at,indexed_at,size_bytes) \
                     VALUES ('A.md','h',0,0,0,1)",
                    [],
                )?;
                let fid: i64 =
                    c.query_row("SELECT id FROM files WHERE path='A.md'", [], |r| r.get(0))?;
                c.execute(
                    "INSERT INTO chunks (file_id,chunk_index,content,char_start,char_end,token_count) \
                     VALUES (?1,0,'text',0,4,1)",
                    [fid],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(count_chunks(&db).await, 1);

        let force = reconcile_embedding_model(&db, root, "bge-m3", 1024)
            .await
            .unwrap();
        assert!(force, "смена модели → force");
        assert_eq!(
            count_chunks(&db).await,
            0,
            "смена модели очистила chunks (§6.5)"
        );
        assert_eq!(
            get_setting(&db, "embedding.dim").await.unwrap().as_deref(),
            Some("1024")
        );
    }
}
