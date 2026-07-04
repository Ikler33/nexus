/**
 * DTO-типы app-домена (F-2d): git-версия сборки, захваченная `build.rs` на компиляции (W-20). Зеркало
 * Rust `BuildInfo` — контракт провода `invoke`. Потребители импортируют по-прежнему из `lib/tauri-api`
 * (barrel-реэкспорт).
 */

/** Git-версия сборки (W-20, зеркалит Rust `BuildInfo`). */
export interface BuildInfo {
  version: string;
  branch: string;
  hash: string;
  dirty: boolean;
}
