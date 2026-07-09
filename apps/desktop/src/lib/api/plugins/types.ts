/**
 * DTO-типы plugins-домена (F-2d): чип права манифеста (DP-8) и статус установленного плагина.
 * Зеркала Rust-структур (`plugin::*`) — контракт провода `invoke`. Потребители импортируют
 * по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Чип права плагина (зеркалит Rust `plugin::PermissionChip`, DP-8): уровень риска для UI. */
export interface PermissionChip {
  kind: string;
  detail: string;
  level: 'safe' | 'caution' | 'sensitive';
}

/**
 * Durable-запись журнала доступа брокера (PLUG-1, зеркалит Rust `plugin::PluginAuditRecord`):
 * персистится в БД (`plugin_audit`) write-before-act на каждый brokered-вызов плагина (THREAT_MODEL
 * T1), переживает рестарт. Контракт команды `list_plugin_audit` (camelCase).
 */
export interface PluginAuditRecord {
  /** id строки БД — стабильный ключ списка + монотонный маркер append-only порядка. */
  id: number;
  /** id плагина из сессии (или '<unknown>' для отозванного токена). */
  pluginId: string;
  /** host-метод (vault.readFile|writeFile|net.fetch|ai.embed|…). */
  method: string;
  /** путь/хост цели (null, если метод без цели). */
  target: string | null;
  /** true = авторизовано (право+scope), false = отказ. */
  allowed: boolean;
  /** текст причины отказа при deny; null при allow. */
  deniedReason: string | null;
  /** unix-сек метки записи. */
  createdAt: number;
}

/** Статус установленного плагина (зеркалит Rust `plugin::PluginInfo`). */
export interface PluginInfo {
  dir: string;
  id: string | null;
  name: string | null;
  version: string | null;
  compatible: boolean;
  error: string | null;
  /** Сводка прав манифеста — чипы и consent-sheet (DP-8). */
  permissions: PermissionChip[];
  /** Включён ли плагин (персист `plugins.<dir>.enabled`, дефолт ВКЛ). Выключенный не открывает сессию. */
  enabled: boolean;
}
