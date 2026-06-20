//! Tauri IPC-команды (типизированный мост к фронту). Фронт вызывает их только через
//! `src/lib/tauri-api.ts` (контракт §4.1).

pub mod agent;
pub mod attachments;
pub mod board;
pub mod chat;
pub mod chat_sessions;
pub mod contradictions;
pub mod debug;
pub mod digest;
pub mod egress;
pub mod episode;
pub mod external;
pub mod git;
pub mod goals;
pub mod graph;
pub mod home;
pub mod inline;
pub mod memory;
pub mod news;
pub mod note_summary;
pub mod plugin;
pub mod properties;
pub mod scheduler;
pub mod search;
pub mod settings;
pub mod suggest;
pub mod tasks;
pub mod vault;
pub mod websearch;
