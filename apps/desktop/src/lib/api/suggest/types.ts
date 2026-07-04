/**
 * DTO-типы suggest-домена (F-2d): предложения связей/похожих заметок (зеркало Rust
 * `suggest::LinkSuggestion`) и авто-тегов (зеркало `tagger::TagSuggestion`). `LinkSuggestion` также
 * результат `news.related` (news-домен импортирует его type-only). Потребители импортируют
 * по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Предложенная связь (зеркалит Rust `suggest::LinkSuggestion`). */
export interface LinkSuggestion {
  path: string;
  title: string | null;
  /** max-sim score (косинус, относительный — для сортировки/порога). */
  score: number;
  /** «Причина» — сниппет лучшего совпавшего чанка целевой заметки. */
  reason: string;
}

/** Предложение авто-тега (AI-2c, зеркалит Rust `tagger::TagSuggestion`). `tags` УЖЕ отфильтрованы по
 *  словарю vault (closed-vocab); `dropped` — сколько модель выдала вне словаря (телеметрия). */
export interface TagSuggestion {
  tags: string[];
  dropped: number;
}
