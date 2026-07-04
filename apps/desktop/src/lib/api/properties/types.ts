/**
 * DTO-типы properties-домена (F-2d): реестр типов свойств (Obsidian Properties, PROP-2) и свойство
 * заметки с разрешённым типом (PROP-3). Зеркала Rust-структур (`properties::*`) — контракт провода
 * `invoke`. Потребители импортируют по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Тип свойства (виджет Properties-панели, PROP-2; зеркалит Rust `properties::PropertyType`). */
export type PropertyType = 'text' | 'list' | 'number' | 'checkbox' | 'date' | 'datetime' | 'tags';
/** Свойство заметки: плоский frontmatter-скаляр + разрешённый тип (реестр+эвристика). */
export interface NoteProperty {
  key: string;
  value: string;
  type: PropertyType;
}
