/**
 * DTO-типы inline-домена (F-2d): события inline-стрима редактора и режим генерации (IL-1/2). Зеркала
 * Rust-структур (`commands::inline::InlineStreamEvent` / `ai::InlineMode`) — контракт провода
 * `invoke`/`Channel`. Потребители импортируют по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Событие inline-стрима редактора (зеркалит Rust `commands::inline::InlineStreamEvent`). Без `sources`
 * — inline не делает RAG-ретрив (D2). Порядок: много `token` → `done` (или `error`). */
export type InlineStreamEvent =
  | { type: 'token'; text: string }
  | { type: 'done'; full: string }
  | { type: 'error'; message: string };

/** Режим inline-генерации (зеркалит Rust `ai::InlineMode`). `prompt` — свободный запрос (⌘/ prompt-box). */
export type InlineMode = 'continue' | 'rewrite' | 'summarize' | 'prompt';
