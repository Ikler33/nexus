//! Nexus desktop — нативный слой (Tauri 2 backend).
//!
//! [`run`] инициализирует структурированное логирование (`tracing`) и Tauri-рантайм.
//! Все IPC-команды регистрируются здесь; фронт обращается к ним исключительно через
//! `src/lib/tauri-api.ts` (контракт §4.1 ARCHITECTURE). По мере роста (срезы Ф0-2+)
//! команды разъезжаются по модулю `commands/` (vault / search / graph / …).

/// Tauri IPC-команды.
mod commands;
/// БД-слой: rusqlite + write-actor + read-pool (WAL) + миграции схемы (ADR-003).
pub mod db;
/// Граф ссылок: беклинки из SQLite (ADR-004).
pub mod graph;
/// Инкрементальный индексатор (files/links/tags) — §4.2.
pub mod indexer;
/// Markdown-парсер (frontmatter, ссылки, теги).
pub mod parser;
/// Поиск по метаданным (title/path/tags) — Ф0.
pub mod search;
/// Глобальное состояние (managed state).
pub mod state;
/// Vault: ленивый листинг + канонизация путей (анти-traversal).
pub mod vault;
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
            commands::search::search_vault,
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
}
