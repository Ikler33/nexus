/**
 * Коннектор v0 (F-8, REFACTOR-PLAN §5) — типизированный минимальный API регистрации, через который
 * МОДУЛИ подключают вклады в ядро. Фундамент под F-9 (news-пилот) и сторонние плагины (решение
 * владельца: ядро/модули с прицелом на плагины).
 *
 * СТРОГИЙ YAGNI-срез (критик §5): ТОЛЬКО `NexusModule {id, activate(ctx)}` + `ModuleContext` с 5
 * реестрами (commands/views/settings/events/api) + `Disposable` + `subscriptions`. НЕ строим (до
 * реальных сторонних плагинов): apiVersion/semver/stability-tiers, capabilities[], deactivate()/
 * hot-unload, динамическую загрузку бандлов, манифест-файлы, песочницу. Существующее —
 * ЛЕГАЛИЗУЕМ (commands-core, mainView-enum F-4, SettingsView.SECTIONS, window-events F-1, lib/api
 * F-2), а не изобретаем заново.
 */
import type { ComponentType } from 'react';
import type { Command, Disposable } from '../commands';
import type { TauriApi } from '../tauri-api';

export type { Disposable };

/**
 * Иконка вклада (ActivityBar/настройки). API-совместима и с lucide-react, и с бренд-глифами
 * (`components/common/BrandGlyphs`) — оба принимают `size` и SVG-пропы.
 */
export type IconComponent = ComponentType<{ size?: number | string; 'aria-hidden'?: boolean }>;

/**
 * Lifecycle-события ядра, на которые модуль подписывается через `ctx.events.on`. НЕ новая шина —
 * тонкая обёртка над существующими сигналами (F-1 window-events `vault:switched` + F-2 доменные
 * подписки `vault:changed`/`jobs:changed`). Список намеренно узкий (YAGNI).
 */
export type CoreEvent = 'vault:opened' | 'vault:changed' | 'jobs:changed';

/**
 * Вклад «полноэкранная main-вью» (реестр `views`, легализация mainView-enum F-4). `id` — значение
 * `MainView` для ядровых вью (`home`/`today`/…/`editor`) либо произвольная строка для тест/сторонних
 * модулей. Питает `MainViewOutlet` (App-lookup) и кнопку ActivityBar.
 */
export interface ViewContribution {
  /** Идентификатор вью = значение mainView-enum (ядро) или строка модуля. App-lookup ключуется им. */
  id: string;
  /** i18n-ключ названия (кнопка ActivityBar, плашка ErrorBoundary). */
  titleKey: string;
  /** Иконка кнопки ActivityBar. */
  icon: IconComponent;
  /** Порядок в ActivityBar / детерминированный порядок реестра (по возрастанию). */
  order: number;
  /** React-компонент вью (рендерится через ErrorBoundary в `MainViewOutlet`). */
  component: ComponentType;
  /** Показывать кнопку в ActivityBar (у редактора — false: вход через дерево/сайдбар). */
  activityBar?: boolean;
  /** Нав-действие по клику в ActivityBar (замыкание над стором — НЕ получает MouseEvent). */
  activate: () => void;
  /** Активна ли вью при данном `mainView` (подсветка кнопки ActivityBar). */
  isActive: (mainView: string) => boolean;
  /** Обернуть в React.Suspense (ленивые вью, напр. AgentView). */
  suspense?: boolean;
}

/**
 * Вклад «секция настроек» (реестр `settings`, легализация `SettingsView.SECTIONS`). Питает левый
 * нав настроек и контент-панель (каждая секция — через ErrorBoundary).
 */
export interface SettingsContribution {
  /** Идентификатор секции = значение `SettingsSection` (ядро) или строка модуля. */
  id: string;
  /** i18n-ключ названия секции. */
  titleKey: string;
  /** Иконка секции в левом наве настроек. */
  icon: IconComponent;
  /** Порядок в наве (по возрастанию). */
  order: number;
  /** React-компонент содержимого секции. */
  component: ComponentType;
}

/** Реестр команд — тонкая обёртка над `commands-core`; префиксует id → `${moduleId}:${id}`. */
export interface CommandsRegistry {
  register(cmd: Command): Disposable;
}

/** Реестр main-вью (поверх mainView-enum F-4). */
export interface ViewsRegistry {
  register(view: ViewContribution): Disposable;
  list(): ViewContribution[];
  get(id: string): ViewContribution | undefined;
}

/** Реестр секций настроек (легализация SECTIONS). */
export interface SettingsRegistry {
  register(section: SettingsContribution): Disposable;
  list(): SettingsContribution[];
}

/** Подписки на lifecycle-события ядра (window/доменные, НЕ новая шина). */
export interface EventsRegistry {
  on(event: CoreEvent, cb: () => void): Disposable;
}

/**
 * Контекст активации модуля — ЕДИНСТВЕННЫЙ вход модуля в ядро (модуль не импортирует ядро-internal
 * напрямую, только через ctx). Всё зарегистрированное авто-трекается в `subscriptions`; ядро
 * dispose'ит их скопом при снятии модуля.
 */
export interface ModuleContext {
  /** Стабильный id модуля (для префикса команд и диагностики). */
  moduleId: string;
  commands: CommandsRegistry;
  views: ViewsRegistry;
  settings: SettingsRegistry;
  events: EventsRegistry;
  /** Типизированный доступ к нативному слою (lib/api F-2), прокинут как есть. */
  api: TauriApi;
  /** Все Disposable, созданные через ctx-реестры (ядро снимает их при dispose модуля). */
  subscriptions: Disposable[];
}

/**
 * Модуль Nexus (коннектор v0). YAGNI: РОВНО `id` + `activate(ctx)`. Никаких version/capabilities/
 * deactivate — до реальных сторонних плагинов (см. docs/dev/connector.md).
 */
export interface NexusModule {
  /** Уникальный id модуля (namespace команд, ключ реестра модулей). */
  id: string;
  /** Точка входа: модуль регистрирует вклады через `ctx`. Вызывается ядром один раз при активации. */
  activate(ctx: ModuleContext): void;
}
