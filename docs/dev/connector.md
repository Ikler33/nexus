# Коннектор модулей v0 (F-8)

Типизированный минимальный API регистрации, через который **модули** подключают вклады в ядро.
Фундамент под F-9 (news-пилот) и будущие **сторонние плагины** (решение владельца: ядро/модули с
прицелом на плагины). Стадия F-8 REFACTOR-PLAN §5.

Код: `apps/desktop/src/lib/connector/`. Каркас (F-8) ЛЕГАЛИЗУЕТ существующие механизмы и даёт
per-contribution изоляцию сбоев. **F-9 вырезал первый реальный модуль — `news`** (см. ниже «Эталон:
как вырезан news»): ядро больше не импортирует `components/news`, вклад идёт через `ctx`.

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
Единая точка регистрации прод-модулей — `connector/modules/index.ts` (`activateModules()`,
импортируется сайд-эффектом из `App.tsx`, как `core-views`). **В проде один модуль — `news`** (F-9);
F-10-серия добавляет свои строкой в `activateModules`. Тест-модуль-заглушка (`isolation.test.tsx`) —
для доказательства изоляции сбоев.

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

## Эталон: как вырезан news (F-9) — ШАБЛОН для F-10

`news` — первый реально вырезанный модуль. Его код-манифест — `connector/modules/news.ts`
(~55 строк). Ровно 3 шага (повторять для каждого модуля F-10-серии):

1. **Манифест модуля** `connector/modules/<feature>.ts`: `export const <feature>Module: NexusModule`
   с `id` и `activate(ctx)`. В `activate` — регистрация вкладов через `ctx`:
   - `ctx.views.register({...})` — main-вью + кнопка ActivityBar (`order/icon/titleKey/activate/isActive`
     переносятся КАК ЕСТЬ из прежних `core-views`, behavior-preserving);
   - `ctx.settings.register({...})` — секция настроек (из прежнего `SettingsView.CORE_SETTINGS_SECTIONS`);
   - `ctx.commands.register({...})` — команды палитры (из прежнего `commands-core`). **id команды
     префиксуется** модулем → `<id>` становится `<feature>:<id>` (у news: `view.news` → `news:view.news`),
     `source=plugin`. Палитра ищет по названию (`titleKey`) — путь пользователя не меняется.
   - `ctx.events.on(...)` / `ctx.api` — если модуль слушает lifecycle-события или ходит в нативный слой.
2. **Регистрация** — одна строка в `connector/modules/index.ts` (`modules.register(<feature>Module)`).
   Больше НИГДЕ (композиционный корень един).
3. **Убрать ядро-ссылки**: удалить вклад фичи из `core-views` (вью), `SettingsView.CORE_SETTINGS_SECTIONS`
   (секция), `commands-core` (команда). После этого **ядро (App/ActivityBar/SettingsView/MainViewOutlet/
   core-views) не импортирует `components/<feature>`** — только реестры отдают вклады.

**Инвариант** (grep-стереж): `grep -rl "components/news" src | grep -v 'components/news/\|modules/news.ts'`
пуст — единственный импортёр `components/news` вне самой фичи — её манифест-модуль. Файл манифеста
живёт вне `src/components/**`, поэтому F-1 линт границ (запрет кросс-импортов между `components/<feature>`)
его не трогает: модуль — легальный слой проводки.

**Что news-модуль оставил в ядре осознанно (v0, документируется — не блокеры вырезания):**
- **i18n-ключи news** (namespace `news.*` ~93 + `settings.news.*` ~23) живут в монолитных
  `i18n/ru.json`/`en.json`. Их вынос в per-module namespace — отдельная фича (i18n-EP), НЕ часть
  вырезания. news-компоненты читают их через `t('news.*')` без изменений. → отложено (F-9b/будущее).
- **`DeadJobsModal` (ядро-chrome) знает job-kind `'newsfeed'`** (`KIND_KEYS`-мапа kind→i18n-ключ, рядом
  с `digest/contradictions/stale_radar/gc/home_widget:`). Это НЕ импорт `components/news` — это
  строковый kind ядровой jobs-инфраструктуры (планировщик — ядровой). Реестр `kind→label` как точка
  вклада модулей — возможная будущая EP; для v0 ядро-знание job-kind'ов приемлемо. → оставлено с
  обоснованием.
- **`stores/news.ts`, `lib/api/news`, `lib/mock/news`** — data/native-слой news, не `components`.
  Инвариант вырезания — про `components/news`; слой данных модуль использует внутри NewsView. Их
  ко-локация под «news-namespace» — косметика, вне скоупа v0.
- **backend-crate `news` (Rust)** — вне скоупа F-9 (сервер-паритет): `commands/chat.rs`/`live_smoke.rs`
  используют `crate::news` — это бэкенд ленты, НЕ фронт-модуль. F-9 вырезает ТОЛЬКО фронт.
- **`NewsSettingsSection.tsx`** физически остаётся в `components/settings/` (делит `SettingsView.module.css`
  — переезд в `components/news/` СОЗДАЛ бы худшую связь news→settings-CSS). Модуль импортирует его
  оттуда; ядро SettingsView его больше не импортирует.

## Как добавить модуль (общий рецепт)
1. Реализовать `NexusModule` (`activate(ctx)` регистрирует вклады через `ctx.*`) — см. эталон news.
2. `modules.register(myModule)` в `connector/modules/index.ts` (`activateAll` уже вызывается там).
3. Вклады автоматически изолированы ErrorBoundary; снятие — `modules.disposeAll()`.
