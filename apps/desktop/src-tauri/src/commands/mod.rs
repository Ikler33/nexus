//! Tauri IPC-команды (типизированный мост к фронту). Фронт вызывает их только через
//! `src/lib/tauri-api.ts` (контракт §4.1).

pub mod graph;
pub mod search;
pub mod vault;
