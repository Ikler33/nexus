import * as mockPlugins from '../../mock/plugins';
import { bridge } from '../bridge';
import type { PluginAuditRecord, PluginInfo } from './types';

/** Дефолтный лимит durable-журнала доступа для UI (бэк зажимает в 1..=AUDIT_MAX_LIMIT=500). */
const AUDIT_DEFAULT_LIMIT = 100;

/**
 * Plugins-домен (F-2d): установленные плагины vault (`.nexus/plugins/*`) — список со статусом
 * совместимости и правами (Ф0-13/Ф2/DP-8), вкл/выкл/удаление, capability-сессия брокера (§7.9) и
 * host-функции через брокер (ADR-002). Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/plugins`);
 * потребители ходят сюда по-прежнему через `tauriApi.plugins` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const plugins = {
  /** Установленные плагины vault (`.nexus/plugins/*`) со статусом совместимости + `enabled` (Ф0-13/Ф2). */
  list: (): Promise<PluginInfo[]> =>
    bridge<PluginInfo[]>('list_plugins', undefined, () => mockPlugins.list()),

  /** Включить/выключить плагин (персист). Выключенный не открывает новую сессию. Вне Tauri — мок. */
  setEnabled: (dir: string, on: boolean): Promise<void> =>
    bridge<void>('set_plugin_enabled', { dir, on }, () => mockPlugins.setEnabled(dir, on)),

  /** Удалить плагин: каталог → в корзину (.nexus/.trash, обратимо) + очистка настроек. Вне Tauri — мок. */
  remove: (dir: string): Promise<void> =>
    bridge<void>('remove_plugin', { dir }, () => mockPlugins.remove(dir)),

  /**
   * Открывает сессию плагина (`.nexus/plugins/<dir>`) → **capability-токен** (§7.9). Токен живёт
   * на host-стороне (в релее), плагину НЕ передаётся (identity по порту/токену, ADR-002).
   */
  openSession: (dir: string): Promise<string> =>
    bridge<string>('plugin_open_session', { dir }, () => mockPlugins.openSession(dir)),

  /**
   * Host-функция плагина через брокер: `authorize` (scope + audit) → dispatch. `method` —
   * `vault.readFile`/`vault.listFiles`/`vault.writeFile`. Результат — JSON (контент/записи/`{ok}`).
   */
  invoke: (token: string, method: string, path?: string, content?: string): Promise<unknown> =>
    bridge<unknown>('plugin_invoke', { token, method, path, content }, () =>
      mockPlugins.invoke(token, method, path, content),
    ),

  /** Закрывает сессию плагина (отзыв токена в брокере). Зовётся при размонтировании плагина. */
  closeSession: (token: string): Promise<void> =>
    bridge<void>('plugin_close_session', { token }, () => mockPlugins.closeSession(token)),

  /**
   * Durable-журнал доступа брокера (PLUG-1, THREAT_MODEL T1): последние `limit` записей из БД
   * (`plugin_audit`), обратно-хронологически (свежие первыми). История переживает рестарт — в отличие
   * от in-session `PluginCall` от iframe. Вне Tauri — мок.
   */
  auditLog: (limit: number = AUDIT_DEFAULT_LIMIT): Promise<PluginAuditRecord[]> =>
    bridge<PluginAuditRecord[]>('list_plugin_audit', { limit }, () =>
      mockPlugins.auditLog(limit),
    ),
};
