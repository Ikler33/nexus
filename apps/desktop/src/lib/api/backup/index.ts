import { invoke } from '@tauri-apps/api/core';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import * as mockBackup from '../../mock/backup';
import { isTauri } from '../bridge';
import type { BackupImportReport } from './types';

/**
 * Backup-домен (F-2d): backup/restore «второго мозга» (#59/W-9) — экспорт/импорт фактов/переписки/
 * эпизодов/навыков в файл. fs делается в доверенном бэкенде; путь выбирает пользователь OS-диалогом.
 * Потребители ходят сюда по-прежнему через `tauriApi.backup` (barrel-реэкспорт в `lib/tauri-api.ts`).
 *
 * Оба метода — честные bridge-исключения (см. шапку `../bridge.ts`): путь с OS-диалогом
 * (`@tauri-apps/plugin-dialog`) — сначала диалог, потом `invoke` — развилка не сводится к паре
 * «команда/мок», поэтому прямой `invoke` с комментом (как `vault.pickDirectory`/`news.exportLogs`).
 */
export const backup = {
  /** Экспорт в файл через save-диалог. Путь сохранённого файла, либо null если отменили. */
  exportToFile: async (): Promise<string | null> => {
    if (!isTauri()) return mockBackup.exportToFile();
    const path = await saveDialog({
      defaultPath: 'orvin-backup.json',
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (!path) return null;
    await invoke<void>('backup_export_to_path', { path });
    return path;
  },
  /** Импорт из файла через open-диалог. Отчёт импорта, либо null если отменили. */
  importFromFile: async (): Promise<BackupImportReport | null> => {
    if (!isTauri()) return mockBackup.importFromFile();
    const path = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (!path || typeof path !== 'string') return null;
    return invoke<BackupImportReport>('backup_import_from_path', { path });
  },
};
