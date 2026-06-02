import { invoke } from '@tauri-apps/api/core';

/**
 * Единственное место в кодовой базе, где разрешён прямой вызов Tauri IPC
 * (`invoke` / `Channel`). Контракт §4.1 ARCHITECTURE: весь фронт обращается к
 * нативному слою только через этот модуль — это «шов», который позволяет вести
 * фронт на моках параллельно бэкенду и держать типы IPC в одном месте.
 *
 * По мере добавления Rust-команд (vault / graph / ai / git) здесь появляются
 * соответствующие типизированные обёртки.
 */

/** Запущены ли мы внутри Tauri-webview (а не в обычном браузере / тесте). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export const tauriApi = {
  app: {
    /** Версия нативного приложения (Rust-команда `app_version`). */
    version: () => invoke<string>('app_version'),
  },
};

export type TauriApi = typeof tauriApi;
