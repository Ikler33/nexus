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

/// Имя каталога плагина из IPC обязано быть РОВНО ОДНИМ нормальным компонентом пути (anti-traversal:
/// `dir` приходит с фронта и используется в `join` + `move_to_trash`, который НЕ проверяет containment).
/// Отвергаем пустое / разделители / `..` / `.` (последнее критично: `dir="."` → `move_to_trash`
/// утащил бы в корзину ВЕСЬ каталог `plugins/`, т.к. его `file_name()` пропускает хвостовой `.`).
fn valid_plugin_dir(dir: &str) -> bool {
    if dir.contains('/') || dir.contains('\\') {
        return false;
    }
    let mut comps = std::path::Path::new(dir).components();
    matches!(comps.next(), Some(std::path::Component::Normal(_))) && comps.next().is_none()
}

/// Резолвит каталог плагина и подтверждает, что он РЕАЛЬНО внутри `.nexus/plugins` (canonicalize →
/// containment). `valid_plugin_dir` проверяет лишь строку — этого мало против подменённой симссылки
/// (`plugins/<dir>` → наружу): canonicalize раскрывает её, а проверка `starts_with` отвергает выход
/// за пределы (defense-in-depth, ревью). Каталог обязан существовать (установленный плагин).
fn resolve_plugin_dir(root: &Path, dir: &str) -> AppResult<std::path::PathBuf> {
    let plugins_root = root.join(".nexus").join("plugins");
    let plugins_canon = plugins_root
        .canonicalize()
        .map_err(|e| AppError::Msg(format!("каталог плагинов недоступен: {e}")))?;
    let canon = plugins_root
        .join(dir)
        .canonicalize()
        .map_err(|e| AppError::Msg(format!("плагин не найден: {e}")))?;
    if !canon.starts_with(&plugins_canon) {
        return Err(AppError::Msg("каталог плагина вне .nexus/plugins".into()));
    }
    Ok(canon)
}

/// Список установленных плагинов vault (`.nexus/plugins/*`) с их статусом совместимости и
/// флагом `enabled` (персист `plugins.<dir>.enabled`, дефолт ВКЛ).
#[tauri::command]
pub async fn list_plugins(state: State<'_, AppState>) -> AppResult<Vec<PluginInfo>> {
    let (root, reader) = {
        let ctx = state.vault().await?;
        (ctx.root.clone(), ctx.db.reader().clone())
    };
    let dir = root.join(".nexus").join("plugins");
    let mut infos = tokio::task::spawn_blocking(move || plugin::scan_plugins(&dir))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?;
    let disabled = plugin::disabled_dirs(&reader).await?;
    for info in &mut infos {
        if disabled.contains(&info.dir) {
            info.enabled = false;
        }
    }
    Ok(infos)
}

/// Включает/выключает плагин (персист `plugins.<dir>.enabled`). Выключенный не открывает новую сессию
/// (`plugin_open_session` отказывает); уже открытые сессии не трогаем (фронт закрывает при размонтаже).
#[tauri::command]
pub async fn set_plugin_enabled(
    state: State<'_, AppState>,
    dir: String,
    on: bool,
) -> AppResult<()> {
    if !valid_plugin_dir(&dir) {
        return Err(AppError::Msg("некорректный каталог плагина".into()));
    }
    let writer = state.vault().await?.db.writer().clone();
    plugin::set_enabled(&writer, &dir, on).await?;
    Ok(())
}

/// Удаляет плагин: каталог `.nexus/plugins/<dir>` → в корзину (`.nexus/.trash`, ОБРАТИМО — не hard rm,
/// владелец: удаление через корзину) + очистка его настроек (переустановка стартует «чистой»).
#[tauri::command]
pub async fn remove_plugin(state: State<'_, AppState>, dir: String) -> AppResult<()> {
    if !valid_plugin_dir(&dir) {
        return Err(AppError::Msg("некорректный каталог плагина".into()));
    }
    let (root, writer) = {
        let ctx = state.vault().await?;
        (ctx.root.clone(), ctx.db.writer().clone())
    };
    // Канонизируем + подтверждаем containment (анти-симссылка) ПЕРЕД move_to_trash (он containment не
    // проверяет). Канон-путь (реальный каталог) и отправляем в корзину.
    let plugin_dir = resolve_plugin_dir(&root, &dir)?;
    let root2 = root.clone();
    tokio::task::spawn_blocking(move || vault::move_to_trash(&root2, &plugin_dir))
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?
        .map_err(|e| AppError::Msg(e.to_string()))?;
    plugin::clear_settings(&writer, &dir).await?;
    Ok(())
}

/// Открывает сессию плагина (`.nexus/plugins/<dir>`): читает манифест, проверяет совместимость,
/// заводит сессию с его scoped-правами в брокере и возвращает **capability-токен** (§7.9). Фронт
/// передаёт токен с каждым `plugin_invoke`. Несовместимый/битый манифест → ошибка (не загружаем).
#[tauri::command]
pub async fn plugin_open_session(state: State<'_, AppState>, dir: String) -> AppResult<String> {
    if !valid_plugin_dir(&dir) {
        return Err(AppError::Msg("некорректный каталог плагина".into()));
    }
    let (root, reader) = {
        let ctx = state.vault().await?;
        (ctx.root.clone(), ctx.db.reader().clone())
    };
    // Выключенный плагин не запускаем (enable/disable, дефолт ВКЛ).
    if !plugin::is_enabled(&reader, &dir).await? {
        return Err(AppError::Msg(format!("плагин выключен: {dir}")));
    }
    // Канонизируем + containment (анти-симссылка: иначе `plugins/<dir>` мог бы указывать наружу, и
    // чтение manifest.json раскрыло бы произвольный JSON-файл — ревью).
    let plugin_dir = resolve_plugin_dir(&root, &dir)?;
    let manifest_path = plugin_dir.join("manifest.json");
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
                ctx.ai.embedder.clone(),
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
        // IP-литерал-хост: metadata/приватный отсекаем ДО сети. Для доменов is_private_host=false —
        // их ловит DNS-гард в dispatch_net (резолв→проверка→пин, анти-rebinding).
        if crate::plugin::blocks_cloud_metadata(&host) || crate::plugin::is_private_host(&host) {
            return Err(format!("SSRF: приватный/metadata-хост запрещён: {host}").into());
        }
        return dispatch_net(url).await.map_err(AppError::Msg);
    }

    // Реальный I/O — вне лока, через тестируемый dispatch.
    dispatch_vault(&vault_root, &method, path.as_deref(), content.as_deref())
        .await
        .map_err(AppError::Msg)
}

/// SSRF-гард плагин-egress: КАЖДЫЙ зарезолвленный IP обязан быть публичным и не-metadata — иначе
/// домен отклоняется ДО коннекта (анти-DNS-rebinding). Пустой резолв — тоже отказ (нечего пинить).
/// Plugin-egress — web-класс (`deny_private=true`). Делегирует общему [`crate::net::check_resolved_ips`]
/// (P0-a, единый источник истины); прежний текст ошибки (адрес не утекает) сохранён.
fn guard_fetch_ips(ips: &[std::net::IpAddr]) -> Result<(), String> {
    crate::net::check_resolved_ips(ips, true).map_err(|_| {
        if ips.is_empty() {
            "dns: пустой резолв".to_string()
        } else {
            "SSRF: хост резолвится в приватный/metadata адрес".to_string()
        }
    })
}

/// `net.fetch`: GET по авторизованному (allowlist) + SSRF-проверенному URL. DNS-гард: резолв хоста →
/// проверка ВСЕХ IP (metadata/приватный) → ПИН проверенного IP в клиент (`resolve_to_addrs`), чтобы
/// между проверкой и коннектом DNS не «перепрыгнул» внутрь сети (rebinding). Без редиректов
/// (анти-redirect-SSRF) и с таймаутом. Возвращает `{status, body}`.
async fn dispatch_net(url: &str) -> Result<serde_json::Value, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "некорректный URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("net.fetch: разрешены только http/https".into());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL без хоста".to_string())?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
    // Резолв → гард → пин (зеркало news::GuardedNewsFetcher::fetch / check_resolved_ips).
    let ips: Vec<std::net::IpAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("dns: {e}"))?
        .map(|sa| sa.ip())
        .collect();
    guard_fetch_ips(&ips)?;
    let pinned = std::net::SocketAddr::new(ips[0], port);
    // egress-lint: allow — PLUGIN-эгресс со СВОЕЙ политикой (broker net-allowlist + SSRF/DNS-гард +
    // таймаут 15с), НЕ core-путь; миграция на net::GuardedClient — отдельный срез (ADR-005-ext, искл.(б)).
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .resolve_to_addrs(&host, &[pinned])
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
            // Атомарно (blocking → spawn_blocking): обрыв в середине записи не оставляет
            // усечённый .md плагином (находка аудита: окно повреждения через tokio::fs::write).
            let bytes = body.as_bytes().to_vec();
            let n = bytes.len();
            tokio::task::spawn_blocking(move || vault::atomic_write_io(&abs, &bytes))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "ok": true, "bytes": n }))
        }
        other => Err(format!("метод пока не поддержан host-стороной: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{Permissions, PluginBroker};
    use std::net::IpAddr;
    use tempfile::TempDir;

    /// Anti-traversal валидатора каталога плагина: принимаем только одиночный нормальный компонент.
    /// КРИТИЧНО отвергаем `.` (иначе remove утащил бы весь `plugins/`), `..`, разделители, абсолютные.
    #[test]
    fn valid_plugin_dir_rejects_traversal() {
        assert!(valid_plugin_dir("hello"));
        assert!(valid_plugin_dir("my-plugin_2"));
        for bad in [
            "",
            ".",
            "..",
            "/",
            "/etc",
            "a/b",
            "../x",
            "a/..",
            "./x",
            ".nexus/..",
            "a/.",
        ] {
            assert!(!valid_plugin_dir(bad), "{bad:?} должен отвергаться");
        }
    }

    /// resolve_plugin_dir: легитимный каталог резолвится внутрь plugins/; симссылка НАРУЖУ отвергается
    /// (containment, анти-симссылка); несуществующий — ошибка.
    #[test]
    fn resolve_plugin_dir_confines_and_rejects_symlink_escape() {
        let v = TempDir::new().unwrap();
        let root = v.path().to_path_buf();
        let plugins = root.join(".nexus").join("plugins");
        std::fs::create_dir_all(plugins.join("good")).unwrap();
        let ok = resolve_plugin_dir(&root, "good").unwrap();
        assert!(ok.ends_with("good"));
        assert!(resolve_plugin_dir(&root, "nope").is_err());

        #[cfg(unix)]
        {
            let outside = TempDir::new().unwrap();
            std::fs::create_dir_all(outside.path().join("secret")).unwrap();
            std::os::unix::fs::symlink(outside.path().join("secret"), plugins.join("evil"))
                .unwrap();
            assert!(
                resolve_plugin_dir(&root, "evil").is_err(),
                "симссылка наружу должна отвергаться"
            );
        }
    }

    /// SSRF-гард plugin-egress: приватные/metadata IP в резолве (вкл. IPv4-mapped) отклоняются;
    /// пустой резолв — отказ; публичные — проходят (находка аудита 2026-06).
    #[test]
    fn guard_fetch_ips_rejects_private_metadata_and_mapped() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.0.31",
            "169.254.169.254",
            "::1",
            "::ffff:192.168.0.1", // IPv4-mapped приватный — обходил гард до фикса
            "::ffff:169.254.169.254", // IPv4-mapped metadata
            "64:ff9b::a9fe:a9fe", // NAT64 → metadata (security-ревью 2026-06)
            "2002:c0a8:1f::",     // 6to4 → 192.168.0.31
            "::a9fe:a9fe",        // IPv4-compatible → metadata
            "100.64.0.1",         // CGNAT 100.64.0.0/10
        ] {
            let a: IpAddr = ip.parse().unwrap();
            assert!(guard_fetch_ips(&[a]).is_err(), "{ip} должен отклоняться");
        }
        assert!(guard_fetch_ips(&[]).is_err(), "пустой резолв — отказ");
        // Публичные проходят; смешанный набор с одним приватным — отказ (ВСЕ обязаны быть публичны).
        assert!(guard_fetch_ips(&["93.184.216.34".parse().unwrap()]).is_ok());
        assert!(guard_fetch_ips(&[
            "93.184.216.34".parse().unwrap(),
            "192.168.1.5".parse().unwrap(),
        ])
        .is_err());
    }

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
