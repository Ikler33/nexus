/**
 * DTO-типы digest-домена (F-2d): дайджест недавних изменений vault (ADR-007 slice 4). Зеркало Rust
 * `digest::Digest` — контракт провода `invoke`. Потребители импортируют по-прежнему из `lib/tauri-api`
 * (barrel-реэкспорт).
 */

/** Дайджест недавних изменений (зеркалит Rust `digest::Digest`, ADR-007 slice 4). Время — Unix-секунды. */
export interface Digest {
  createdAt: number;
  since: number;
  content: string;
  noteCount: number;
}
