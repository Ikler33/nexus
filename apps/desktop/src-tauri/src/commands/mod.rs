//! Tauri IPC-команды (типизированный мост к фронту). Фронт вызывает их только через
//! `src/lib/tauri-api.ts` (контракт §4.1).

pub mod chat;
pub mod graph;
pub mod plugin;
pub mod search;
pub mod suggest;
pub mod vault;
