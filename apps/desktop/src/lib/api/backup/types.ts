/**
 * DTO-типы backup-домена (F-2d): отчёт импорта бэкапа «второго мозга» (#59/W-9). Зеркало Rust
 * `backup::ImportReport` — контракт провода `invoke`. Потребители импортируют по-прежнему из
 * `lib/tauri-api` (barrel-реэкспорт).
 */

/** Отчёт импорта бэкапа (#59, зеркалит Rust `backup::ImportReport`). */
export interface BackupImportReport {
  factsAdded: number;
  factsSkipped: number;
  sessionsAdded: number;
  sessionsReused: number;
  messagesAdded: number;
  messagesSkipped: number;
  episodesAdded: number;
  episodesSkipped: number;
  skillsAdded: number;
  skillsSkipped: number;
  messagesOrphaned: number;
  episodesOrphaned: number;
  schemaVersionMismatch: boolean;
}
