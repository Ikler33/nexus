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
/// БД-слой: rusqlite + write-actor + read-pool (WAL) + миграции схемы (ADR-003).
pub mod db;
/// Eval-харнесс качества RAG (golden + recall@k/nDCG/MRR + baseline) — §6.6.
pub mod eval;
/// git-sync (Фаза 3, §8): vault как git-репозиторий — фундамент (open/init, .gitignore, status).
pub mod git;
/// Граф ссылок: беклинки из SQLite (ADR-004).
pub mod graph;
/// Инкрементальный индексатор (files/links/tags) — §4.2.
pub mod indexer;
/// Markdown-парсер (frontmatter, ссылки, теги).
pub mod parser;
/// Plugin loader (минимум): manifest + совместимость версии API (без broker — Ф2).
pub mod plugin;
/// Поиск по метаданным (title/path/tags) — Ф0.
pub mod search;
/// Глобальное состояние (managed state).
pub mod state;
/// Предложения связей (режим 1 max-sim) — §6.
pub mod suggest;
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
        .invoke_handler(tauri::generate_handler![
            app_version,
            commands::vault::open_vault,
            commands::vault::list_dir,
            commands::vault::read_file,
            commands::vault::write_file,
            commands::vault::list_notes,
            commands::graph::get_backlinks,
            commands::graph::get_local_graph,
            commands::search::search_vault,
            commands::search::search_content,
            commands::chat::chat_rag,
            commands::chat::chat_cancel,
            commands::suggest::get_link_suggestions,
            commands::plugin::list_plugins,
            commands::plugin::plugin_open_session,
            commands::plugin::plugin_invoke,
            commands::plugin::plugin_close_session,
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
