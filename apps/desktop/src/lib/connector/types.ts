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
import type { UIState } from '../../stores/ui';

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

/**
 * Точка монтирования оверлея в дереве оболочки (F-10d, МИНИМАЛЬНО — НЕ универсальная mount-система,
 * YAGNI). ДВЕ точки, обе питает `OverlayOutlet` (по инстансу на точку):
 * - `'app'` (по умолчанию) — уровень `.app`, поверх всей оболочки (титлбар/тело/статусбар). Так
 *   рендерятся 8 оверлеев F-8c/F-10b/F-10c (все `position:fixed`, стекинг по z-index).
 * - `'appBody'` — ВНУТРЬ тела `.appBody` (между титлбаром и статусбаром). Единственный потребитель —
 *   `graph` (F-10d): его слой `position:absolute; inset:0` НЕ должен покрывать хром (фикс владельца
 *   «хром торчал поверх графа»). `.app`-точка (не позиционирована) якорила бы граф к вьюпорту.
 */
export type OverlayMount = 'app' | 'appBody';

/**
 * Вклад «оверлей» (реестр `overlays`, F-8c — легализация 7 хардкод-строк App.tsx `{xOpen && <Panel/>}`:
 * goals/memory/episodes/tasks/inbox/digest/contradictions). НЕ полноэкранная вью (`views`), а
 * плавающая/модальная панель поверх тела: своя видимость (`isOpen`-селектор), свой Esc/close внутри
 * компонента. Питает `OverlayOutlet` — рендерит открытые оверлеи, КАЖДЫЙ через ErrorBoundary.
 *
 * `isOpen` — селектор из ui-стора (v0-коупл: оверлеи ядра управляются `*Open`-булями `UIState`;
 * store-agnostic абстракция состояния оверлея отложена вместе с прочим north-star плагинов, YAGNI —
 * см. docs/dev/connector.md). F-8c — ТОЛЬКО реестр+outlet; перенос `*Open`-стейта в модули — F-10b.
 */
export interface OverlayContribution {
  /** Идентификатор оверлея (ключ реестра, `key` ErrorBoundary). */
  id: string;
  /** i18n-ключ имени панели (плашка ErrorBoundary «модуль X упал»). */
  titleKey: string;
  /** Селектор видимости из ui-стора: `(s) => s.goalsOpen`. Читает существующие були (F-8c). */
  isOpen: (state: UIState) => boolean;
  /** Порядок рендера в OverlayOutlet (по возрастанию) — сохраняет прежний DOM-порядок App.tsx. */
  order: number;
  /** React-компонент панели (рендерится через ErrorBoundary в `OverlayOutlet`). */
  component: ComponentType;
  /**
   * Точка монтирования (F-10d). По умолчанию `'app'` — 8 оверлеев его НЕ задают, их DOM/стекинг
   * байт-идентичны. `graph` задаёт `'appBody'` (слой внутри тела, не покрывает хром). См. `OverlayMount`.
   */
  mount?: OverlayMount;
}

/**
 * Точка стыковки workspace-панели в теле оболочки (F-12, МИНИМАЛЬНО — НЕ универсальная dock-система,
 * YAGNI). Панель докается в `.appBody` в одной из ТРЁХ позиций (pref `aiLayout`, ядро-chrome):
 * - `'side'` — колонка сбоку от редактора (рефлоу грида `.appBody`, НЕ float);
 * - `'bottom'` — панель снизу (рефлоу грида);
 * - `'overlay'` — плавающая панель поверх тела со скримом.
 * Ровно те три, что уже принимает `AiPanel` (`variant`). Позицию/видимость выбирает ЯДРО (App),
 * компонент даёт модуль — см. `PanelContribution` и docs/dev/connector.md «### F-12».
 */
export type PanelPlacement = 'side' | 'bottom' | 'overlay';

/**
 * Вклад «workspace-панель» (реестр `panels`, F-12 — легализация хардкода App.tsx `import { AiPanel }`
 * + 3-вариантный рендер). НЕ полноэкранная вью (`views`: взаимоисключаемый `mainView`) и НЕ оверлей
 * (`overlays`: единый `isOpen`-селектор поверх `UIState`, `<Component/>` без пропов): панель СОСУЩЕСТВУЕТ
 * с main-вью «Редактор», её позиция ведётся pref `aiLayout` (3 варианта `PanelPlacement`), а видимость —
 * составным ядровым выражением (`chatOpen && !reading && mainView==='editor'`, ui+derived) + рефлоу грида
 * `.appBody` и CSS-переменные размера. Всё это — ядро-chrome (как `mount`/позиционирование у оверлея);
 * модуль даёт ТОЛЬКО компонент. Питает `AiPanelOutlet` (рендер через per-contribution ErrorBoundary).
 */
export interface PanelContribution {
  /** Идентификатор панели (ключ реестра, `key` ErrorBoundary). */
  id: string;
  /** i18n-ключ имени панели (плашка ErrorBoundary «модуль X упал»). */
  titleKey: string;
  /**
   * React-компонент панели, принимающий позицию докинга (`variant`). Рендерится через ErrorBoundary
   * в `AiPanelOutlet`; App передаёт `variant` из pref `aiLayout` (ядро-chrome).
   */
  component: ComponentType<{ variant?: PanelPlacement }>;
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

/** Реестр оверлеев (F-8c — легализация 7 хардкод-строк App.tsx, питает OverlayOutlet). */
export interface OverlaysRegistry {
  register(overlay: OverlayContribution): Disposable;
  list(): OverlayContribution[];
  get(id: string): OverlayContribution | undefined;
}

/** Реестр workspace-панелей (F-12 — питает AiPanelOutlet; в проде один вклад — chat/AiPanel). */
export interface PanelsRegistry {
  register(panel: PanelContribution): Disposable;
  list(): PanelContribution[];
  get(id: string): PanelContribution | undefined;
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
  /** Реестр оверлеев (F-8c): модуль регистрирует плавающую/модальную панель — вырезание F-10b. */
  overlays: OverlaysRegistry;
  /** Реестр workspace-панелей (F-12): модуль регистрирует докаемую панель тела (chat/AiPanel). */
  panels: PanelsRegistry;
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
