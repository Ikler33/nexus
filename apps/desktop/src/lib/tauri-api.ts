import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import * as mockVault from './mock/vault';

/**
 * Единственное место в кодовой базе, где разрешён прямой вызов Tauri IPC
 * (`invoke` / `Channel`) — контракт §4.1 ARCHITECTURE. Весь фронт ходит к нативному
 * слою только через `tauriApi`.
 *
 * Вне Tauri (браузерное превью, vitest) методы прозрачно проксируются в мок-бэкенд
 * (`./mock/*`) — это позволяет вести фронт/дизайн на тех же контрактах параллельно
 * бэкенду (DESIGN §0).
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

/** Запущены ли мы внутри Tauri-webview (а не в обычном браузере / тесте). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export const tauriApi = {
  app: {
    /** Версия нативного приложения (Rust-команда `app_version`). */
    version: () => (isTauri() ? invoke<string>('app_version') : Promise.resolve('dev')),
  },

  vault: {
    /** Открывает vault по абсолютному пути; в браузере — мок. */
    openVault: (path: string) =>
      isTauri() ? invoke<VaultInfo>('open_vault', { path }) : mockVault.openVault(path),

    /** Ленивый листинг каталога (`dirPath` относительный; '' = корень). */
    listDir: (dirPath: string) =>
      isTauri() ? invoke<FileEntry[]>('list_dir', { dirPath }) : mockVault.listDir(dirPath),

    /** Системный выбор папки vault (нативный диалог Tauri). Вне Tauri — `null`. */
    pickDirectory: async (): Promise<string | null> => {
      if (!isTauri()) return null;
      const picked = await openDialog({ directory: true, multiple: false });
      return typeof picked === 'string' ? picked : null;
    },
  },
};

export type TauriApi = typeof tauriApi;
