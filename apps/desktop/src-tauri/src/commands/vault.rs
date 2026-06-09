//! Команды vault: открытие хранилища и ленивый листинг каталогов.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::OptionalExtension;
use tauri::State;

use crate::ai::{
    self, ChatProvider, EmbeddingProvider, LocalConfig, OpenAiChatProvider, OpenAiEmbedder,
};
use crate::db::Database;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, VaultContext};
use crate::vault::{self, FileEntry, NoteRef, VaultInfo};
use crate::vector::VectorIndex;

/// Открывает vault: канонизирует папку, открывает БД в `.nexus/nexus.db`, сохраняет в state.
#[tauri::command]
pub async fn open_vault(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    path: String,
) -> AppResult<VaultInfo> {
    let root = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| AppError::Msg(format!("vault path: {e}")))?;
    if !root.is_dir() {
        return Err("vault: путь не является каталогом".into());
    }

    let db = Database::open(root.join(".nexus").join("nexus.db")).await?;

    let info = VaultInfo {
        root: root.to_string_lossy().into_owned(),
        name: vault::vault_name(&root),
    };

    // Конфиг `.nexus/local.json` парсим ОДИН раз (раньше — дважды: build_rag + build_chat), кросс-план #8.
    let local_cfg = load_local_config(&root).await;

    // RAG (Ф1-5): строим эмбеддер + векторный индекс. Если конфига нет / нет embedding-секции /
    // сервер недоступен — vault открывается без AI (local-first).
    let rag = match &local_cfg {
        Some(cfg) => build_rag(&root, &db, cfg).await,
        None => None,
    };
    let (vectors, embedder, indexer) = match rag {
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

    // Chat-провайдеры (ADR-005): пара — обычный с reasoning (RAG-чат, точность) + «быстрый» без
    // reasoning (примитивы R2: inline/дайджест/судья). Строятся вместе (есть/нет синхронно).
    let (chat, chat_fast) = match &local_cfg {
        Some(cfg) => match build_chat(cfg).await {
            Some((normal, fast)) => (Some(normal), Some(fast)),
            None => (None, None),
        },
        None => (None, None),
    };

    // Запускаем watcher + фоновую индексацию (начальный скан + инкрементальные события).
    crate::indexer::spawn(indexer, app.clone());

    // Планировщик фоновых задач (ADR-007): встроенный kind `gc` (самоочистка) + (если есть chat) `digest`
    // (LLM-дайджест недавних изменений, #35). Воркер живёт, пока открыт vault.
    let mut registry = crate::scheduler::default_registry(db.writer().clone());
    // Дайджест/судья — это примитивы: берут «быстрый» chat без reasoning (R2).
    if let Some(fast) = &chat_fast {
        let handler: Arc<dyn crate::scheduler::JobHandler> =
            Arc::new(crate::digest::DigestHandler::new(
                db.reader().clone(),
                fast.clone(),
                db.writer().clone(),
            ));
        registry.insert(crate::digest::KIND_DIGEST.to_string(), handler);
    }
    // «Поиск противоречий» (#vision) — нужен chat И векторы (пары-кандидаты по эмбеддингам → судья).
    if let (Some(fast), Some(vectors)) = (&chat_fast, &vectors) {
        let handler: Arc<dyn crate::scheduler::JobHandler> =
            Arc::new(crate::contradictions::ContradictionHandler::new(
                db.reader().clone(),
                vectors.clone(),
                fast.clone(),
                db.writer().clone(),
            ));
        registry.insert(crate::contradictions::KIND_CONTRA.to_string(), handler);
    }
    // Recurring (slice 6): LLM-фичи сами переназначаются после прогона — авто-обновление раз в сутки
    // (совпадает с их окном «недавнего»). С backpressure (S5) фон не мешает интерактиву.
    const DAY_SECS: i64 = 24 * 3600;
    let mut recurring: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    if chat.is_some() {
        recurring.insert(crate::digest::KIND_DIGEST.to_string(), DAY_SECS);
    }
    if chat.is_some() && vectors.is_some() {
        recurring.insert(crate::contradictions::KIND_CONTRA.to_string(), DAY_SECS);
    }
    // On-change (slice 7): те же LLM-kind перезапускаются после правок vault (с дебаунсом).
    let on_change: Vec<String> = recurring.keys().cloned().collect();
    crate::scheduler::spawn_worker(
        db.writer().clone(),
        app,
        Arc::new(registry),
        recurring,
        db.reader().clone(),
        on_change,
    );
    // Seed: gc на ближайший тик; дайджест — если просрочен (run-if-overdue, S2) и chat сконфигурирован.
    let _ = crate::scheduler::enqueue(db.writer(), crate::scheduler::KIND_GC, "", 0, 3).await;
    if chat.is_some()
        && crate::digest::should_generate(db.reader())
            .await
            .unwrap_or(false)
    {
        let _ = crate::scheduler::enqueue(db.writer(), crate::digest::KIND_DIGEST, "", 0, 2).await;
    }
    // Поиск противоречий — run-if-overdue (нужны и chat, и векторы).
    if chat.is_some()
        && vectors.is_some()
        && crate::contradictions::should_generate(db.reader())
            .await
            .unwrap_or(false)
    {
        let _ =
            crate::scheduler::enqueue(db.writer(), crate::contradictions::KIND_CONTRA, "", 0, 2)
                .await;
    }

    *state.vault.write().await = Some(VaultContext {
        root,
        db,
        vectors,
        embedder,
        chat,
        chat_fast,
    });
    tracing::info!(vault = %info.root, "opened vault");
    Ok(info)
}

/// Читает и парсит `.nexus/local.json` ОДИН раз (кросс-план #8 — раньше парсили дважды). `None` —
/// конфига нет / битый JSON (AI отключается, vault работает без AI — local-first).
async fn load_local_config(root: &Path) -> Option<LocalConfig> {
    let raw = tokio::fs::read_to_string(root.join(".nexus").join("local.json"))
        .await
        .ok()?;
    LocalConfig::parse(&raw)
        .map_err(|e| tracing::warn!(error = %e, "local.json: разбор не удался — AI отключён"))
        .ok()
}

/// Строит RAG-подсистему из распарсенного конфига. `None` — нет embedding-секции / сервер недоступен
/// (RAG отключается, vault работает без AI). Делает реконсиляцию модели (§6.5).
async fn build_rag(
    root: &Path,
    db: &Database,
    cfg: &LocalConfig,
) -> Option<(Arc<dyn EmbeddingProvider>, Arc<VectorIndex>, bool)> {
    let emb = cfg.ai.embedding.as_ref()?;
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

/// Строит пару chat-провайдеров из конфига (`ai.chat`): `(обычный с reasoning, быстрый без reasoning)`.
/// `None`, если секции нет или клиент не инициализировался. Доступность сервера здесь НЕ проверяем —
/// это выяснится при первом стриме. Оба — тот же сервер/модель; быстрый шлёт `enable_thinking=false` (R2).
async fn build_chat(cfg: &LocalConfig) -> Option<(Arc<dyn ChatProvider>, Arc<dyn ChatProvider>)> {
    let chat = cfg.ai.chat.as_ref()?;
    let model = chat.model.clone().unwrap_or_else(|| "chat".to_string());
    let build = || {
        OpenAiChatProvider::new(&chat.url, &model, None)
            .map_err(|e| tracing::warn!(error = %e, "chat-провайдер не инициализирован"))
            .ok()
    };
    let normal = build()?;
    let fast = build()?.without_reasoning();
    tracing::info!(model = %model, "chat-провайдеры включены (reasoning + fast)");
    Some((Arc::new(normal), Arc::new(fast)))
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
pub async fn list_dir(state: State<'_, AppState>, dir_path: String) -> AppResult<Vec<FileEntry>> {
    // Копируем корень и отпускаем лок: ФС-обход уводим в blocking-пул.
    let root = state.vault().await?.root.clone();
    let entries = tokio::task::spawn_blocking(move || vault::list_dir(&root, Path::new(&dir_path)))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?;
    Ok(entries?)
}

/// Читает содержимое файла vault (путь относительный; анти-traversal через resolve).
#[tauri::command]
pub async fn read_file(state: State<'_, AppState>, path: String) -> AppResult<String> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path(&root, Path::new(&path))?;
    Ok(tokio::fs::read_to_string(&abs).await?)
}

/// Пишет содержимое файла vault (целевой путь может ещё не существовать).
#[tauri::command]
pub async fn write_file(
    state: State<'_, AppState>,
    path: String,
    content: String,
) -> AppResult<()> {
    let root = current_root(&state).await?;
    let abs = vault::resolve_vault_path_for_write(&root, Path::new(&path))?;
    Ok(tokio::fs::write(&abs, content).await?)
}

/// Все заметки vault (path + title) — для автокомплита `[[wikilink]]` и поиска.
#[tauri::command]
pub async fn list_notes(state: State<'_, AppState>) -> AppResult<Vec<NoteRef>> {
    let reader = state.vault().await?.db.reader().clone();
    Ok(reader
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
        .await?)
}

/// Корень текущего открытого vault (или [`AppError::NoVault`], если не открыт).
async fn current_root(state: &State<'_, AppState>) -> AppResult<PathBuf> {
    Ok(state.vault().await?.root.clone())
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
