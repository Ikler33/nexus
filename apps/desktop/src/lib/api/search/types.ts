/**
 * DTO-типы search-домена (F-2d): результат гибридного поиска по телу (вектор + FTS5 (+граф) → RRF).
 * Зеркало Rust `search::SearchHit` — контракт провода `invoke`. `SearchHit` также payload события
 * `sources` чат-стрима (chat-домен импортирует его type-only). Потребители импортируют по-прежнему
 * из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Результат гибридного поиска по телу (зеркалит Rust `search::SearchHit`). */
export interface SearchHit {
  chunkId: number;
  path: string;
  title: string | null;
  headingPath: string | null;
  snippet: string;
  /** Слитый RRF-score (вектор + FTS); шкала относительная, для сортировки. */
  score: number;
}
