/**
 * DTO-типы vault-домена (F-2a): файловое дерево, сведения о vault, ссылки на заметки, теги.
 * Зеркала Rust-структур — контракт провода `invoke`. Потребители импортируют их по-прежнему
 * из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Запись файлового дерева (зеркалит Rust `vault::FileEntry`). */
export interface FileEntry {
  name: string;
  /** Путь относительно корня vault, разделитель `/`. */
  path: string;
  isDir: boolean;
  hasChildren: boolean;
  sizeBytes: number;
}

/** Сведения об открытом vault (зеркалит Rust `vault::VaultInfo`). */
export interface VaultInfo {
  root: string;
  name: string;
}

/** Лёгкая ссылка на заметку (зеркалит Rust `vault::NoteRef`) — для автокомплита/поиска. */
export interface NoteRef {
  path: string;
  title: string | null;
}

/** Тег с количеством заметок (зеркалит Rust `tags::TagCount`, DP-2 — панель «Теги»). */
export interface TagCount {
  name: string;
  count: number;
}
