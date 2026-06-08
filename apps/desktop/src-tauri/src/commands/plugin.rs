//! Команды плагинов: Ф0-13 (манифесты + совместимость) + Ф2-2b (capability-broker live):
//! `plugin_open_session` (выдать токен по правам манифеста) и `plugin_invoke` (host-функция через
//! брокер: авторизация по токену + scoped-проверка + audit → dispatch). §7.4/§7.9, ADR-002.

use std::path::Path;

use tauri::State;

use crate::ai::EmbeddingProvider;
use crate::error::{AppError, AppResult};
use crate::plugin::{self, ApiRequest, CapToken, PluginInfo, PluginSession};
use crate::search;
use crate::state::AppState;
use crate::vault;
use crate::vector::VectorIndex;

/// Список установленных плагинов vault (`.nexus/plugins/*`) с их статусом совместимости.
#[tauri::command]
pub async fn list_plugins(state: State<'_, AppState>) -> AppResult<Vec<PluginInfo>> {
    let root = state.vault().await?.root.clone();
    let dir = root.join(".nexus").join("plugins");
    tokio::task::spawn_blocking(move || plugin::scan_plugins(&dir))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))
}

/// Открывает сессию плагина (`.nexus/plugins/<dir>`): читает манифест, проверяет совместимость,
/// заводит сессию с его scoped-правами в брокере и возвращает **capability-токен** (§7.9). Фронт
/// передаёт токен с каждым `plugin_invoke`. Несовместимый/битый манифест → ошибка (не загружаем).
#[tauri::command]
pub async fn plugin_open_session(state: State<'_, AppState>, dir: String) -> AppResult<String> {
    let root = state.vault().await?.root.clone();
    let manifest_path = root
        .join(".nexus")
        .join("plugins")
        .join(&dir)
        .join("manifest.json");
    let json = tokio::fs::read_to_string(&manifest_path)
        .await
        .map_err(|e| AppError::Msg(format!("manifest: {e}")))?;
    let manifest = plugin::load_manifest(&json, plugin::CORE_API_VERSION)
        .map_err(|e| AppError::Msg(e.to_string()))?;

    let session = PluginSession {
        id: manifest.id,
        permissions: manifest.permissions,
        vault_root: root,
    };
    let token = state
        .plugins
        .lock()
        .map_err(|_| "broker lock")?
        .open_session(session);
    Ok(token.as_str().to_string())
}

/// Закрывает сессию плагина: мгновенно отзывает токен в брокере (§7.9). Вызывается фронтом при
/// размонтировании плагина (закрытие панели/iframe) — иначе сессии копятся в брокере. Идемпотентно.
#[tauri::command]
pub async fn plugin_close_session(state: State<'_, AppState>, token: String) -> AppResult<()> {
    let token = CapToken::from_ipc(token);
    state
        .plugins
        .lock()
        .map_err(|_| "broker lock")?
        .revoke(&token);
    Ok(())
}

/// Host-функция плагина через брокер (§7.4): авторизация по токену + scoped-проверка + audit, затем
/// dispatch. Методы: `vault.readFile`/`listFiles` (`vault:read`), `vault.writeFile` (`vault:write`),
/// `ui.*` (только авторизация — регистрацию делает фронт), `ai.embed`/`ai.searchSemantic` (`ai:embed`),
/// `net.fetch` (`net`-allowlist + SSRF-гард; URL в `path`). `content` несёт текст/запрос для `ai.*`.
/// Результат — JSON (контент / записи / `{ok}` / вектор / хиты / `{status,body}`).
#[tauri::command]
pub async fn plugin_invoke(
    state: State<'_, AppState>,
    token: String,
    method: String,
    path: Option<String>,
    content: Option<String>,
) -> AppResult<serde_json::Value> {
    let token = CapToken::from_ipc(token);

    // Для `net.fetch` хост берём из URL (в `path`) — он нужен брокеру для allowlist-проверки.
    let net_host = if method == "net.fetch" {
        path.as_deref()
            .and_then(|u| reqwest::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_string))
    } else {
        None
    };

    // Авторизация (синхронно, под локом) → достаём vault_root, лок отпускаем до async I/O.
    let vault_root = {
        let mut broker = state.plugins.lock().map_err(|_| "broker lock")?;
        let req = ApiRequest {
            method: &method,
            path: path.as_deref(),
            host: net_host.as_deref(),
        };
        broker.authorize(&token, &req).map_err(|e| e.to_string())?;
        broker
            .session(&token)
            .ok_or("сессия не найдена")?
            .vault_root
            .clone()
    };

    // `ui.*` — только авторизация (фактическую регистрацию делает фронт-реестр команд); host-I/O нет.
    if method.starts_with("ui.") {
        return Ok(serde_json::Value::Bool(true));
    }

    // `ai.*` — нужен открытый vault с эмбеддером. Снимаем reader/vectors/embedder под read-локом и
    // отпускаем его ДО сетевого эмбеддинга (как в `search_content`). Текст/запрос — в `content`.
    if method.starts_with("ai.") {
        let (reader, vectors, embedder) = {
            let ctx = state.vault().await?;
            (
                ctx.db.reader().clone(),
                ctx.vectors.clone(),
                ctx.embedder.clone(),
            )
        };
        let embedder = embedder.ok_or("эмбеддер не сконфигурирован")?;
        return dispatch_ai(
            &reader,
            vectors.as_deref(),
            embedder.as_ref(),
            &method,
            content.as_deref(),
        )
        .await
        .map_err(AppError::Msg);
    }

    // `net.fetch` — egress по allowlist (проверен брокером выше) + SSRF-гард: даже разрешённый хост
    // не должен указывать на приватный/loopback/metadata-адрес.
    if method == "net.fetch" {
        let url = path.as_deref().ok_or("нет аргумента path (url)")?;
        let host = net_host.ok_or("некорректный URL")?;
        if crate::plugin::is_private_host(&host) {
            return Err(format!("SSRF: приватный/loopback хост запрещён: {host}").into());
        }
        return dispatch_net(url).await.map_err(AppError::Msg);
    }

    // Реальный I/O — вне лока, через тестируемый dispatch.
    dispatch_vault(&vault_root, &method, path.as_deref(), content.as_deref())
        .await
        .map_err(AppError::Msg)
}

/// `net.fetch`: GET по уже авторизованному (allowlist) + SSRF-проверенному URL. Без следования
/// редиректам (анти-redirect-SSRF) и с таймаутом. Возвращает `{status, body}`.
async fn dispatch_net(url: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "status": status, "body": body }))
}

/// ai-вызовы плагина (право `ai:embed`): эмбеддинг текста (`ai.embed`) и семантический поиск по vault
/// (`ai.searchSemantic`). Текст/запрос — в `content`. Тестируется напрямую (MockEmbedder + temp-индекс).
/// `ai.complete` (стрим) — отдельным срезом (стриминг по порту). См. BACKLOG.
async fn dispatch_ai(
    reader: &crate::db::ReadPool,
    vectors: Option<&VectorIndex>,
    embedder: &dyn EmbeddingProvider,
    method: &str,
    input: Option<&str>,
) -> Result<serde_json::Value, String> {
    let text = input.ok_or("нет аргумента content")?;
    match method {
        "ai.embed" => {
            let vec = embedder
                .embed_query(text)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(vec).map_err(|e| e.to_string())
        }
        "ai.searchSemantic" => {
            let hits = search::hybrid_search(
                reader,
                vectors,
                Some(embedder),
                text.to_string(),
                search::SearchOptions {
                    limit: 8,
                    filter: None,
                    center: None,
                },
            )
            .await
            .map_err(|e| e.to_string())?;
            serde_json::to_value(hits).map_err(|e| e.to_string())
        }
        other => Err(format!("ai-метод пока не поддержан host-стороной: {other}")),
    }
}

/// Реальный I/O авторизованного vault-вызова. Отдельная (тестируемая) функция: брокер уже проверил
/// право+scope, здесь — резолв пути (та же анти-traversal граница, defense-in-depth) и сам I/O.
/// `vault.onFileChanged` — подписка на события, не invoke-метод (придёт отдельным каналом).
async fn dispatch_vault(
    vault_root: &Path,
    method: &str,
    path: Option<&str>,
    content: Option<&str>,
) -> Result<serde_json::Value, String> {
    match method {
        "vault.readFile" => {
            let p = path.ok_or("нет аргумента path")?;
            let abs =
                vault::resolve_vault_path(vault_root, Path::new(p)).map_err(|e| e.to_string())?;
            let text = tokio::fs::read_to_string(&abs)
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::String(text))
        }
        "vault.listFiles" => {
            // Пустой путь = корень vault. `list_dir` сам резолвит rel (та же граница), скрывает
            // служебное/dotfiles, не рекурсивен. Синхронный I/O → spawn_blocking.
            let root = vault_root.to_path_buf();
            let rel = std::path::PathBuf::from(path.unwrap_or(""));
            let entries = tokio::task::spawn_blocking(move || vault::list_dir(&root, &rel))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            serde_json::to_value(entries).map_err(|e| e.to_string())
        }
        "vault.writeFile" => {
            let p = path.ok_or("нет аргумента path")?;
            let body = content.ok_or("нет аргумента content")?;
            // Запись: канонизируем РОДИТЕЛЯ (файл может не существовать) — `*_for_write`.
            let abs = vault::resolve_vault_path_for_write(vault_root, Path::new(p))
                .map_err(|e| e.to_string())?;
            tokio::fs::write(&abs, body)
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "ok": true, "bytes": body.len() }))
        }
        other => Err(format!("метод пока не поддержан host-стороной: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{Permissions, PluginBroker};
    use tempfile::TempDir;

    fn vault() -> TempDir {
        let d = TempDir::new().unwrap();
        std::fs::create_dir(d.path().join("Notes")).unwrap();
        std::fs::write(d.path().join("Notes/a.md"), "# A\nтекст").unwrap();
        d
    }

    #[tokio::test]
    async fn dispatch_reads_lists_writes_within_vault() {
        let v = vault();
        let root = v.path().canonicalize().unwrap();

        let r = dispatch_vault(&root, "vault.readFile", Some("Notes/a.md"), None)
            .await
            .unwrap();
        assert_eq!(r.as_str().unwrap(), "# A\nтекст");

        let r = dispatch_vault(&root, "vault.listFiles", Some(""), None)
            .await
            .unwrap();
        let names: Vec<&str> = r
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"Notes"));

        let r = dispatch_vault(&root, "vault.writeFile", Some("Notes/new.md"), Some("hi"))
            .await
            .unwrap();
        assert_eq!(r["ok"], serde_json::json!(true));
        assert_eq!(
            std::fs::read_to_string(root.join("Notes/new.md")).unwrap(),
            "hi"
        );
    }

    #[tokio::test]
    async fn dispatch_blocks_path_escape() {
        let v = vault();
        let root = v.path().canonicalize().unwrap();
        assert!(
            dispatch_vault(&root, "vault.readFile", Some("../../etc/passwd"), None)
                .await
                .is_err()
        );
        assert!(
            dispatch_vault(&root, "vault.readFile", Some("/etc/passwd"), None)
                .await
                .is_err()
        );
        assert!(
            dispatch_vault(&root, "vault.writeFile", Some("../evil.md"), Some("x"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn dispatch_unknown_method_and_missing_args() {
        let v = vault();
        let root = v.path().canonicalize().unwrap();
        assert!(
            dispatch_vault(&root, "vault.deleteEverything", Some("x"), None)
                .await
                .is_err()
        );
        assert!(dispatch_vault(&root, "vault.readFile", None, None)
            .await
            .is_err());
        assert!(dispatch_vault(&root, "vault.writeFile", Some("a.md"), None)
            .await
            .is_err());
    }

    /// E2E-логика `plugin_invoke` без Tauri: брокер авторизует scope → dispatch делает I/O.
    /// Проверяет связку «scope (broker) + boundary+I/O (dispatch)» и аудит (allow+deny).
    #[tokio::test]
    async fn broker_scope_then_dispatch_end_to_end() {
        async fn invoke(
            broker: &mut PluginBroker,
            token: &CapToken,
            root: &Path,
            method: &str,
            path: Option<&str>,
            content: Option<&str>,
        ) -> Result<serde_json::Value, String> {
            let req = ApiRequest {
                method,
                path,
                host: None,
            };
            broker.authorize(token, &req).map_err(|e| e.to_string())?;
            dispatch_vault(root, method, path, content).await
        }

        let v = vault();
        let root = v.path().canonicalize().unwrap();
        let mut broker = PluginBroker::new();
        let permissions: Permissions =
            serde_json::from_str(r#"{"vault:read":["Notes/**"],"vault:write":["Notes/**"]}"#)
                .unwrap();
        let token = broker.open_session(PluginSession {
            id: "demo".into(),
            permissions,
            vault_root: root.clone(),
        });

        // read/write в scope — ок; вне scope — брокер отказывает ДО I/O.
        assert!(invoke(
            &mut broker,
            &token,
            &root,
            "vault.readFile",
            Some("Notes/a.md"),
            None
        )
        .await
        .is_ok());
        assert!(invoke(
            &mut broker,
            &token,
            &root,
            "vault.readFile",
            Some("Secrets/x.md"),
            None
        )
        .await
        .is_err());
        assert!(invoke(
            &mut broker,
            &token,
            &root,
            "vault.writeFile",
            Some("Notes/b.md"),
            Some("hi")
        )
        .await
        .is_ok());
        assert!(invoke(
            &mut broker,
            &token,
            &root,
            "vault.writeFile",
            Some("Secrets/b.md"),
            Some("hi")
        )
        .await
        .is_err());

        let audit = broker.audit().entries();
        assert!(audit.iter().any(|e| e.allowed));
        assert!(audit.iter().any(|e| !e.allowed));
    }

    #[tokio::test]
    async fn dispatch_ai_embed_and_search() {
        use crate::ai::MockEmbedder;
        use crate::eval::{index_corpus, GoldenDoc};
        use std::sync::Arc;

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder { dim: 16 });
        let vectors = Arc::new(VectorIndex::open(root.join(".nexus/v.usearch"), 16).unwrap());
        let docs = vec![
            GoldenDoc {
                path: "a.md".into(),
                body: "# Кошки\nкошки и собаки".into(),
            },
            GoldenDoc {
                path: "b.md".into(),
                body: "# Rust\nownership and borrow".into(),
            },
        ];
        let db = index_corpus(&root, &docs, embedder.clone(), vectors.clone())
            .await
            .unwrap();

        // ai.embed → вектор длины dim.
        let v = dispatch_ai(
            db.reader(),
            Some(&vectors),
            embedder.as_ref(),
            "ai.embed",
            Some("hi"),
        )
        .await
        .unwrap();
        assert_eq!(v.as_array().unwrap().len(), 16);

        // ai.searchSemantic → непустая выдача (лексический хвост FTS5 ловит «кошки»).
        let hits = dispatch_ai(
            db.reader(),
            Some(&vectors),
            embedder.as_ref(),
            "ai.searchSemantic",
            Some("кошки"),
        )
        .await
        .unwrap();
        assert!(!hits.as_array().unwrap().is_empty());

        // нет аргумента / неизвестный ai-метод → ошибка.
        assert!(dispatch_ai(
            db.reader(),
            Some(&vectors),
            embedder.as_ref(),
            "ai.embed",
            None
        )
        .await
        .is_err());
        assert!(dispatch_ai(
            db.reader(),
            Some(&vectors),
            embedder.as_ref(),
            "ai.summarize",
            Some("x")
        )
        .await
        .is_err());
    }
}
