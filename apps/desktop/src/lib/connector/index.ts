/**
 * Barrel коннектора v0 (F-8). Единая точка импорта каркаса модулей: типы контракта, глобальные
 * реестры вкладов, менеджер модулей, обёртка lifecycle-событий. Что входит в v0 и что отложено до
 * сторонних плагинов — docs/dev/connector.md.
 */
export type {
  CommandsRegistry,
  CoreEvent,
  Disposable,
  EventsRegistry,
  IconComponent,
  ModuleContext,
  NexusModule,
  OverlayContribution,
  OverlayMount,
  OverlaysRegistry,
  SettingsContribution,
  SettingsRegistry,
  ViewContribution,
  ViewsRegistry,
} from './types';
export { overlayRegistry, settingsRegistry, viewRegistry } from './registries';
export { modules } from './module-manager';
export { onCoreEvent } from './events';
export { registerCoreViews } from './core-views';
// core-overlays удалён (F-10b): все 7 оверлеев вырезаны в модули (`connector/modules/*`); реестр
// `overlays` теперь питается ТОЛЬКО модулями через `ctx.overlays` (см. docs/dev/connector.md).
