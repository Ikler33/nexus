/**
 * DTO-типы contradictions-домена (F-2d): найденное противоречие пары заметок (#vision, спека
 * `docs/specs/contradictions.md`). Зеркало Rust `contradictions::Contradiction` — контракт провода
 * `invoke`. Потребители импортируют по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Найденное противоречие (зеркалит Rust `contradictions::Contradiction`). `ctype` — hard|soft|temporal. */
export interface Contradiction {
  pathA: string;
  pathB: string;
  ctype: string;
  explanation: string;
  createdAt: number;
}
