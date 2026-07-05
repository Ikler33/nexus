# Коннектор модулей v0 (F-8)

Типизированный минимальный API регистрации, через который **модули** подключают вклады в ядро.
Фундамент под F-9 (news-пилот) и будущие **сторонние плагины** (решение владельца: ядро/модули с
прицелом на плагины). Стадия F-8 REFACTOR-PLAN §5.

Код: `apps/desktop/src/lib/connector/`. **Behavior-preserving каркас** — ни один модуль пока не
вырезан из ядра (это F-9); коннектор лишь ЛЕГАЛИЗУЕТ уже существующие механизмы и даёт per-contribution
изоляцию сбоев.

## Что есть в v0

### `NexusModule` + `ModuleContext`
- `NexusModule { id: string; activate(ctx: ModuleContext): void }` — РОВНО это (YAGNI).
- `ModuleContext` — единственный вход модуля в ядро (модуль не импортирует ядро-internal напрямую):
  - `commands` — тонкая обёртка над `commands-core` (`lib/commands.ts`); id команды префиксуется
    `${moduleId}:${id}`, source=`plugin` (приоритет хоткеев пользователь > плагин > ядро уже есть).
  - `views` — реестр полноэкранных main-вью **поверх mainView-enum F-4** (`stores/ui.ts`). Питает
    `MainViewOutlet` (App-lookup активной вью) и кнопки ActivityBar (order/icon/titleKey/activate/isActive).
  - `settings` — реестр секций настроек (легализация `SettingsView.SECTIONS`). Питает левый нав и
    контент-панель настроек.
  - `events` — подписка на lifecycle-события ядра: `vault:opened` / `vault:changed` / `jobs:changed`.
    Это **обёртка над существующими каналами**, НЕ новая шина: `vault:opened` → window-событие
    `vault:switched` (F-1), `vault:changed`/`jobs:changed` → доменные подписки F-2
    (`tauriApi.events.*`).
  - `api` — типизированный `lib/api/<домены>` (F-2), прокинут как есть (`tauriApi`).
  - `subscriptions: Disposable[]` — всё зарегистрированное авто-трекается; ядро dispose'ит скопом.

### Реестр модулей
`modules.register(m)` (одно место) + `modules.activateAll()` — **детерминированный** порядок
активации (= порядок регистрации). `modules.disposeAll()` снимает все вклады всех модулей.
**В проде реальных модулей ноль** (каркас). Тест-модуль-заглушка используется только в
`isolation.test.tsx` для доказательства изоляции.

### ErrorBoundary per-contribution
`components/common/ErrorBoundary.tsx` — каждая зарегистрированная вью (`MainViewOutlet`) и секция
настроек (`SettingsSectionOutlet`) оборачивается React-ErrorBoundary: рендер-сбой вклада показывает
плашку «модуль X упал» + reload вместо белого экрана. `CommandRegistry.run` — в try/catch (упавшая
команда → тост, не висящий reject). Цель владельца: **«ИИ правит модуль → app не падает»**. Доказано
`src/lib/connector/isolation.test.tsx` (падающая вью тест-модуля → app жив + плашка).

## Легализация (что было — что стало)
| Реестр    | Было (ядро)                              | Стало (через коннектор)                         |
|-----------|------------------------------------------|-------------------------------------------------|
| commands  | `commands-core` + `commands` registry    | `ctx.commands` — та же registry, id в namespace |
| views     | тернарник App.tsx + хардкод ActivityBar  | `viewRegistry` (core-views) → MainViewOutlet + ActivityBar |
| settings  | массив `SECTIONS` в SettingsView         | `settingsRegistry` → нав + SettingsSectionOutlet |
| events    | россыпь `useEffect` + window/tauri        | `onCoreEvent` (обёртка тех же каналов)          |
| api       | `tauriApi`                                | `ctx.api` (тот же объект)                        |

## Отложено (YAGNI — до реальных сторонних плагинов)
Осознанно НЕ строим в v0 (критик REFACTOR-PLAN §5):
- `apiVersion` / semver / stability-tiers / `Experimental<T>`;
- `capabilities[]`-декларация вкладов;
- `deactivate()` / hot-unload (есть `disposeAll`, но не пер-модульный live-цикл);
- динамическая загрузка бандлов, манифест-файлы модулей;
- песочница исполнения модулей.

## Отложено в F-8b (скоуп-дисциплина среза)
- **Миграция фича-эффектов App.tsx на `ctx.events`.** Реестр `events` ЕСТЬ и покрыт тестами
  (`events.test.ts`), но существующие `useEffect` в `App.tsx` (goals-reload по `vault:changed`,
  digest/contradictions-refetch по `jobs:changed`, episodic/aiFeatures-sync по смене vault) пока
  подписываются напрямую — их перевод на `onCoreEvent` behavior-preserving, но раздувал бы срез.
  Каркас доказан тестом изоляции без этой миграции.

## Как добавить модуль (когда придёт F-9 / плагины)
1. Реализовать `NexusModule` (`activate(ctx)` регистрирует вклады через `ctx.*`).
2. `modules.register(myModule)` в композиционном корне + `modules.activateAll()` на старте.
3. Вклады автоматически изолированы ErrorBoundary; снятие — `modules.disposeAll()`.
