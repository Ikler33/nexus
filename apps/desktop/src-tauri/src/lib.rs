//! Nexus desktop — нативный слой (Tauri 2 backend).
//!
//! [`run`] инициализирует структурированное логирование (`tracing`) и Tauri-рантайм.
//! Все IPC-команды регистрируются здесь; фронт обращается к ним исключительно через
//! `src/lib/tauri-api.ts` (контракт §4.1 ARCHITECTURE). По мере роста (срезы Ф0-2+)
//! команды разъезжаются по модулю `commands/` (vault / search / graph / …).

/// AI-слой: раздельные Chat/Embedding провайдеры (ADR-005).
pub mod ai;
/// Markdown-чанкер для RAG (§6.1).
pub mod chunker;
/// Tauri IPC-команды.
mod commands;
/// «Поиск противоречий» (#vision): фоновый LLM-kind — пары-кандидаты → судья → таблица `contradictions`.
pub mod contradictions;
/// Локальный crash-reporter: panic-hook → scrubbed-лог в `~/.nexus/crashes/` (Ф4-14).
pub mod crash;
/// БД-слой: rusqlite + write-actor + read-pool (WAL) + миграции схемы (ADR-003).
pub mod db;
/// «Дайджест изменений» (#35): первый LLM-kind планировщика (суммаризация недавних заметок).
pub mod digest;
/// Единый тип ошибки командного слоя (кросс-план #9): доменные ошибки через `?`, JS видит строку.
pub mod error;
/// Eval-харнесс качества RAG (golden + recall@k/nDCG/MRR + baseline) — §6.6.
pub mod eval;
/// git-sync (Фаза 3, §8): vault как git-репозиторий — фундамент (open/init, .gitignore, status).
pub mod git;
/// «Прогресс целей» (#35): кросс-файловый список заметок-целей (#goal) — vision-волна 2.
pub mod goals;
/// Граф ссылок: беклинки из SQLite (ADR-004).
pub mod graph;
/// HOME-дашборд (бэкенд): агрегация виджетов (stats/recent/goals; LLM-виджеты — H2+).
pub mod home;
/// Инкрементальный индексатор (files/links/tags) — §4.2.
pub mod indexer;
/// Egress-граница ядра (ADR-005-ext): `GuardedClient` + политика + audit — единый chokepoint HTTP.
pub mod net;
/// Лента новостей (спека `docs/specs/news-feed.md`): NF-1 — парсеры фидов + keyword-фильтр.
pub mod news;
/// Markdown-парсер (frontmatter, ссылки, теги).
pub mod parser;
/// Plugin loader (минимум): manifest + совместимость версии API (без broker — Ф2).
pub mod plugin;
/// `Redacted<T>`: безопасные Debug/Display (контент/пути не утекают в логи по неосторожности) — AC-SEC-6.
pub mod redact;
/// Планировщик фоновых задач (ADR-007): очередь `jobs` (слой данных — slice 1).
pub mod scheduler;
/// Поиск по метаданным (title/path/tags) — Ф0.
pub mod search;
/// Глобальное состояние (managed state).
pub mod state;
/// Предложения связей (режим 1 max-sim) — §6.
pub mod suggest;
/// Теги vault: список с количеством для панели «Теги» сайдбара (DP-2).
pub mod tags;
/// Vault: ленивый листинг + канонизация путей (анти-traversal).
pub mod vault;
/// Векторный ANN-индекс (usearch HNSW) — §6.1/§6.2.
pub mod vector;
/// Файловый watcher (debounce + ignore + нормализация по пути).
pub mod watcher;

/// Возвращает версию приложения из `CARGO_PKG_VERSION`.
///
/// Первая сквозная IPC-команда — служит дымовым тестом моста фронт ↔ Rust.
#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Точка входа: настраивает логирование и запускает event loop Tauri.
pub fn run() {
    // Локальный crash-reporter до всего остального (Ф4-14): паники → scrubbed-лог в ~/.nexus/crashes/.
    crash::install_hook();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "starting Nexus desktop"
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::new())
        // E5 (срез 2 net.md): политика эгресса переживает рестарт — app-local `egress.json`
        // в OS config-dir (вне vault/git и вне keychain). Нет файла/битый → local-first-дефолты.
        .setup(|app| {
            use tauri::Manager;
            if let Ok(dir) = app.path().app_config_dir() {
                let saved = net::load_egress_state(&dir.join("egress.json"));
                let st = app.state::<state::AppState>();
                st.apply_egress_state(&saved);
                // NF-4 (AC-NF-7): NewsFeed-фича и "news"-allowlist — производные от news.json
                // (единственная истина consent); восстанавливаем на старте.
                let news_cfg = news::load_news_config(&dir.join("news.json"));
                news::sync_egress_policy(&st.egress_policy, &news_cfg);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            commands::egress::get_egress_state,
            commands::egress::set_egress_offline,
            commands::egress::set_egress_feature,
            commands::vault::open_vault,
            commands::vault::list_dir,
            commands::vault::read_file,
            commands::vault::write_file,
            commands::vault::list_notes,
            commands::vault::resolve_note,
            commands::vault::list_tags,
            commands::vault::rescan_vault,
            commands::vault::notes_count,
            commands::vault::file_mtime,
            commands::graph::get_backlinks,
            commands::graph::get_local_graph,
            commands::graph::get_full_graph,
            commands::search::search_vault,
            commands::search::search_content,
            commands::chat::chat_rag,
            commands::chat::chat_cancel,
            commands::inline::inline_complete,
            commands::inline::inline_cancel,
            commands::news::get_news,
            commands::news::news_mark_read,
            commands::news::news_to_note,
            commands::news::refresh_news,
            commands::news::get_news_config,
            commands::news::set_news_config,
            commands::news::news_sources,
            commands::news::news_article,
            commands::news::news_summarize,
            commands::suggest::get_link_suggestions,
            commands::suggest::get_related_notes,
            commands::goals::list_goals,
            commands::home::get_home_data,
            commands::home::get_home_activity,
            commands::home::get_widget,
            commands::home::refresh_widget,
            commands::home::get_stale_radar,
            commands::home::refresh_stale_radar,
            commands::digest::get_latest_digest,
            commands::digest::generate_digest,
            commands::contradictions::get_contradictions,
            commands::contradictions::generate_contradictions,
            commands::scheduler::get_job_counts,
            commands::scheduler::job_active,
            commands::settings::get_ai_config,
            commands::settings::set_ai_config,
            commands::settings::test_ai_connection,
            commands::plugin::list_plugins,
            commands::plugin::plugin_open_session,
            commands::plugin::plugin_invoke,
            commands::plugin::plugin_close_session,
            commands::git::git_status,
            commands::git::git_commit,
            commands::git::git_commit_paths,
            commands::git::git_set_token,
            commands::git::git_clear_token,
            commands::git::git_has_token,
            commands::git::git_set_remote,
            commands::git::git_get_remote,
            commands::git::git_sync,
            commands::git::git_merge_preview,
            commands::git::git_resolve_conflicts,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Nexus desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_version_matches_cargo_pkg_version() {
        assert_eq!(app_version(), env!("CARGO_PKG_VERSION"));
    }

    /// AC-SEC-5 (каркас): строгий CSP без unsafe-inline/eval + минимальные capabilities
    /// (никаких широких fs/shell/http прав — vault-доступ идёт через собственные команды,
    /// не через fs-плагин). Регрессия: ужесточение каркаса не должно молча откатываться.
    #[test]
    fn csp_and_capabilities_are_hardened() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"]
            .as_str()
            .expect("CSP должен быть задан");
        assert!(
            !csp.contains("unsafe-inline"),
            "CSP: запрещён unsafe-inline"
        );
        assert!(!csp.contains("unsafe-eval"), "CSP: запрещён unsafe-eval");
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("object-src 'none'"));

        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        let perms = caps["permissions"].as_array().expect("permissions");
        for p in perms {
            let s = p.as_str().unwrap_or("");
            assert!(!s.starts_with("fs:"), "широкое fs-право запрещено: {s}");
            assert!(!s.starts_with("shell:"), "shell-право запрещено: {s}");
            assert!(!s.starts_with("http:"), "http-право запрещено: {s}");
        }
    }
}
