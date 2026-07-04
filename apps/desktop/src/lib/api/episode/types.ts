/**
 * DTO-типы episode-домена (F-2d): эпизодическая память (EP) — саммари прошлых сессий для ретривала
 * (EP-2) и панели (EP-3). Зеркала Rust-структур (`episode::*`) — контракт провода `invoke`.
 * `EpisodeHit` также payload события `episodeSources` чат-стрима (chat-домен импортирует его
 * type-only). Потребители импортируют по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Эпизод памяти (EP-2, зеркалит Rust `episode::EpisodeHit`) — саммари прошлой сессии. По клику грузит
 *  сессию (`sessionId`). `summarySnippet` — обрезанное саммари; `started/endedAt` — unix-секунды. */
export interface EpisodeHit {
  episodeId: number;
  sessionId: number;
  sessionTitle: string;
  summarySnippet: string;
  startedAt: number;
  endedAt: number;
  score: number;
}

/** Эпизод для панели (EP-3, зеркалит Rust `episode::EpisodeRow`) — полная строка + темы + флаг
 *  скрытия. `topics` — распарсенный JSON; `dismissed` — скрыт из ретривала (обратимо). */
export interface EpisodeRow {
  id: number;
  sessionId: number;
  sessionTitle: string;
  summary: string;
  topics: string[];
  startedAt: number;
  endedAt: number;
  generatedAt: number;
  dismissed: boolean;
}
