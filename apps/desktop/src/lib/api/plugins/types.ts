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
