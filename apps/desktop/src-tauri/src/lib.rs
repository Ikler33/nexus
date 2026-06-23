//! Nexus desktop — нативный слой (Tauri 2 backend).
//!
//! [`run`] инициализирует структурированное логирование (`tracing`) и Tauri-рантайм.
//! Все IPC-команды регистрируются здесь; фронт обращается к ним исключительно через
//! `src/lib/tauri-api.ts` (контракт §4.1 ARCHITECTURE). По мере роста (срезы Ф0-2+)
//! команды разъезжаются по модулю `commands/` (vault / search / graph / …).

// Ядро (CORE-1): db/parser/vector/plugin/vault/redact/chunker/net/ai извлечены в крейт `nexus-core`.
// Ре-экспортим под теми же именами, чтобы существующие `crate::net::…`, `crate::db::…`,
// `crate::ai::…` и т.д. по всему приложению (commands, error.rs, indexer, …) резолвились без правки
// call-site (low-churn-стратегия среза). Будущий headless agent-service использует `nexus_core::*`.
// CORE-1c-1: кластер индекса/ретривала (watcher/tags/tagger/indexer/graph/suggest/search) тоже
// переехал в ядро — ре-экспортим, чтобы `crate::indexer::…`/`crate::search::…`/… по приложению
// (commands, home, board, …) резолвились без правки. Индексатор отвязан от Tauri (IndexerHooks);
// desktop строит эмит-колбэки в `commands::vault::open_vault` и зовёт `indexer::events::spawn`.
// CORE-1c-2: кластер памяти/движка (chat_log/contradictions/episode/eval/memory/relation_reasons/
// starting_questions) тоже в ядре — ре-экспортим, чтобы `crate::memory::…`/`crate::eval::…`/… по
// приложению (commands::memory/episode/contradictions, …) резолвились без правки call-site.
pub use nexus_core::{
    ai, backup, chat_log, chunker, contradictions, db, episode, eval, graph, indexer, memory, net,
    parser, plugin, redact, relation_reasons, search, starting_questions, suggest, tagger, tags,
    vault, vector, watcher,
};

/// Канбан-доска (BOARD-2, спека `docs/specs/kanban-board.md`): выборка заметок-задач (frontmatter `status`).
pub mod board;
/// Tauri IPC-команды.
mod commands;
/// Локальный crash-reporter: panic-hook → scrubbed-лог в `~/.nexus/crashes/` (Ф4-14).
pub mod crash;
/// «Дайджест изменений» (#35): первый LLM-kind планировщика (суммаризация недавних заметок).
pub mod digest;
/// Единый тип ошибки командного слоя (кросс-план #9): доменные ошибки через `?`, JS видит строку.
pub mod error;
/// git-sync (Фаза 3, §8): vault как git-репозиторий — фундамент (open/init, .gitignore, status).
pub mod git;
/// «Прогресс целей» (#35): кросс-файловый список заметок-целей (#goal) — vision-волна 2.
pub mod goals;
/// HOME-дашборд (бэкенд): агрегация виджетов (stats/recent/goals; LLM-виджеты — H2+).
pub mod home;
/// Лента новостей (спека `docs/specs/news-feed.md`): NF-1 — парсеры фидов + keyword-фильтр.
pub mod news;
/// Реестр типов свойств (PROP-2, спека §7): `.nexus/property-types.json` + эвристика (Obsidian Properties).
pub mod properties;
/// Планировщик фоновых задач (ADR-007): очередь `jobs` (слой данных — slice 1).
pub mod scheduler;
/// Глобальное состояние (managed state).
pub mod state;
pub mod websearch;

/// Live-smoke LLM-этапов на прод-серверах (тесты игнорируются по умолчанию, см. модуль).
#[cfg(test)]
mod live_smoke;

/// Возвращает версию приложения из `CARGO_PKG_VERSION`.
///
/// Первая сквозная IPC-команда — служит дымовым тестом моста фронт ↔ Rust.
#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Git-версия сборки (W-20): ветка, короткий хеш, флаг «грязного» дерева + версия пакета.
/// Значения захвачены `build.rs` на этапе компиляции (см. `NEXUS_GIT_*`). Статусбар рисует
/// `ветка @ хеш`, чтобы в самом приложении было видно, ЧТО запущено.
#[derive(serde::Serialize)]
struct BuildInfo {
    version: String,
    branch: String,
    hash: String,
    dirty: bool,
}

#[tauri::command]
fn app_build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        branch: env!("NEXUS_GIT_BRANCH").to_string(),
        hash: env!("NEXUS_GIT_HASH").to_string(),
        dirty: env!("NEXUS_GIT_DIRTY") == "1",
    }
}

/// Guard non-blocking-писателя файлового лога: живёт до конца процесса (дроп = потеря хвоста лога).
static LOG_GUARD: std::sync::OnceLock<tracing_appender::non_blocking::WorkerGuard> =
    std::sync::OnceLock::new();

/// Каталог файлового лога отладки: `<data_local>/app.nexus.desktop/logs` (macOS —
/// `~/Library/Application Support/app.nexus.desktop/logs`). Режим отладки (запрос владельца
/// 2026-06-11): «кликнул — ничего не произошло» нечем ловить без персистентного журнала.
fn log_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("app.nexus.desktop").join("logs"))
}

/// Точка входа: настраивает логирование (stdout + файловый журнал с ротацией по дням) и запускает
/// event loop Tauri. В файл идёт то же, что в stdout, ПЛЮС UI-события фронта (`log_ui_event`):
/// только ИМЕНА действий и метаданные, никакого контента заметок/вопросов (принцип AC-SEC-6).
pub fn run() {
    // Локальный crash-reporter до всего остального (Ф4-14): паники → scrubbed-лог в ~/.nexus/crashes/.
    crash::install_hook();

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let file_layer = log_dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        let (writer, guard) =
            tracing_appender::non_blocking(tracing_appender::rolling::daily(dir, "nexus.log"));
        LOG_GUARD.set(guard).ok();
        Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer),
        )
    });
    tracing_subscriber::registry()
        .with(tracing_subscriber::filter::LevelFilter::INFO)
        .with(tracing_subscriber::fmt::layer())
        .with(file_layer)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        log_dir = %log_dir().map(|d| d.display().to_string()).unwrap_or_default(),
        "starting Nexus desktop"
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
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
                // W-1 (W2): Web-фича и "web"-allowlist — производные от websearch.json
                // (consent = сохранённый URL SearXNG); восстанавливаем на старте.
                let web_cfg = websearch::config::load(&dir.join("websearch.json"));
                websearch::config::sync_egress_policy(&st.egress_policy, &web_cfg);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            app_build_info,
            commands::egress::get_egress_state,
            commands::egress::set_egress_offline,
            commands::egress::set_egress_feature,
            commands::vault::open_vault,
            commands::vault::list_dir,
            commands::vault::read_file,
            commands::vault::read_file_meta,
            commands::vault::file_hash,
            commands::vault::write_file,
            commands::vault::set_frontmatter_field,
            commands::vault::delete_path,
            commands::vault::rename_path,
            commands::vault::list_versions,
            commands::vault::read_version,
            commands::vault::list_notes,
            commands::tasks::list_tasks,
            commands::attachments::write_attachment,
            commands::attachments::read_attachment,
            commands::attachments::resolve_attachment,
            commands::vault::resolve_note,
            commands::vault::list_tags,
            commands::vault::notes_by_tag,
            commands::vault::rescan_vault,
            commands::vault::notes_count,
            commands::vault::file_mtime,
            commands::graph::get_backlinks,
            commands::graph::get_unlinked_mentions,
            commands::graph::get_local_graph,
            commands::graph::get_full_graph,
            commands::search::search_vault,
            commands::search::search_content,
            commands::chat::chat_rag,
            commands::chat::chat_cancel,
            commands::agent::agent_run,
            commands::agent::agent_approve,
            commands::agent::agent_pause,
            commands::agent::agent_resume,
            commands::agent::agent_cancel,
            commands::agent::agent_undo,
            commands::backup::backup_export_json,
            commands::backup::backup_import_json,
            commands::chat_sessions::chat_sessions_list,
            commands::chat_sessions::chat_search,
            commands::chat_sessions::chat_session_messages,
            commands::chat_sessions::chat_log_exchange,
            commands::chat_sessions::chat_delete_last_exchange,
            commands::chat_sessions::chat_session_to_note,
            commands::memory::memory_list,
            commands::memory::memory_add,
            commands::memory::memory_propose,
            commands::memory::memory_set_pinned,
            commands::memory::memory_edit,
            commands::memory::memory_delete,
            commands::memory::memory_fact_history,
            commands::memory::memory_consolidate_plan,
            commands::memory::memory_consolidate_apply,
            commands::memory::memory_consolidate_undo,
            commands::episode::episode_list,
            commands::episode::episode_dismiss,
            commands::episode::episode_restore,
            commands::episode::episode_purge,
            commands::episode::episode_get_enabled,
            commands::episode::episode_set_enabled,
            commands::inline::inline_complete,
            commands::inline::inline_cancel,
            commands::note_summary::get_note_summary,
            commands::news::get_news,
            commands::news::news_mark_read,
            commands::news::news_to_note,
            commands::news::news_related,
            commands::news::refresh_news,
            commands::news::get_news_config,
            commands::news::set_news_config,
            commands::news::news_allow_host,
            commands::news::news_disallow_host,
            commands::websearch::get_websearch_config,
            commands::websearch::set_websearch_config,
            commands::news::news_sources,
            commands::news::news_article,
            commands::news::news_summarize,
            commands::suggest::get_link_suggestions,
            commands::suggest::get_related_notes,
            commands::suggest::explain_relation,
            commands::suggest::get_starting_questions,
            commands::suggest::suggest_tags,
            commands::goals::list_goals,
            commands::board::list_board,
            commands::board::get_board,
            commands::board::save_board,
            commands::board::list_boards,
            commands::board::stale_tasks,
            commands::properties::get_property_types,
            commands::properties::set_property_type,
            commands::properties::get_note_properties,
            commands::home::get_home_data,
            commands::home::get_home_activity,
            commands::home::get_widget,
            commands::home::refresh_widget,
            commands::home::get_stale_radar,
            commands::home::refresh_stale_radar,
            commands::home::insights_get_enabled,
            commands::home::insights_set_enabled,
            commands::digest::get_latest_digest,
            commands::digest::generate_digest,
            commands::contradictions::get_contradictions,
            commands::contradictions::generate_contradictions,
            commands::contradictions::contradictions_get_enabled,
            commands::contradictions::contradictions_set_enabled,
            commands::scheduler::get_job_counts,
            commands::scheduler::job_active,
            commands::scheduler::get_dead_jobs,
            commands::scheduler::get_active_jobs,
            commands::scheduler::restart_scheduler,
            commands::debug::log_ui_event,
            commands::scheduler::retry_dead_job,
            commands::scheduler::clear_dead_jobs,
            commands::settings::get_ai_config,
            commands::settings::set_ai_config,
            commands::settings::set_agent_flags,
            commands::settings::test_ai_connection,
            commands::plugin::list_plugins,
            commands::plugin::set_plugin_enabled,
            commands::plugin::remove_plugin,
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
            commands::external::open_external,
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

    #[test]
    fn build_info_carries_version_and_git_fields() {
        let bi = app_build_info();
        assert_eq!(bi.version, env!("CARGO_PKG_VERSION"));
        // branch/hash берутся из build.rs; в CI/локально это git-репо → непустые. Если git
        // недоступен (релиз без .git) — пустые строки допустимы, поэтому проверяем лишь тип/наличие.
        assert_eq!(bi.dirty, env!("NEXUS_GIT_DIRTY") == "1");
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
