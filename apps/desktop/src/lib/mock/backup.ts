import type { BackupImportReport } from '../tauri-api';

/**
 * Мок backup/restore (#59) для браузер-превью/тестов вне Tauri: файловых диалогов нет, поэтому
 * экспорт «возвращает» фиктивный путь, а импорт — нулевой отчёт (ничего не добавлено). Зеркалит
 * контракт команд (строка-путь | null; `BackupImportReport` | null).
 */
export async function exportToFile(): Promise<string | null> {
  return 'orvin-backup.json';
}

export async function importFromFile(): Promise<BackupImportReport | null> {
  return {
    factsAdded: 0,
    factsSkipped: 0,
    sessionsAdded: 0,
    sessionsReused: 0,
    messagesAdded: 0,
    messagesSkipped: 0,
    episodesAdded: 0,
    episodesSkipped: 0,
    skillsAdded: 0,
    skillsSkipped: 0,
    messagesOrphaned: 0,
    episodesOrphaned: 0,
    schemaVersionMismatch: false,
  };
}
