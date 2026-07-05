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
  - `overlays` (F-8c) — реестр **оверлеев** (плавающих/модальных панелей поверх тела: goals/memory/
    episodes/tasks/inbox/digest/contradictions). Легализация 7 хардкод-строк App.tsx `{xOpen &&
    <Panel/>}`. Питает `OverlayOutlet`. См. «### Реестр оверлеев (F-8c)» ниже.
  - `subscriptions: Disposable[]` — всё зарегистрированное авто-трекается; ядро dispose'ит скопом.

### Реестр оверлеев (F-8c)
Расширение коннектора под **серию F-10** (разведка F-10a: 7 модулей — оверлеи, не main-вью). F-8
отложил preview/inspector/statusBar-реестры «до первого модуля, который потребует»; F-10a — этот
момент (7 однотипных оверлеев не ложатся на 5 реестров v0). Добавлен РОВНО оверлей-реестр (YAGNI:
никаких preview/inspector/statusBar «заодно»).

- **`overlays.register({ id, titleKey, isOpen, order, component })`** (`OverlayContribution`) — Map-
  реестр ПО ОБРАЗЦУ `viewRegistry`: `list()` детерминирован (сортировка по `order`), `get(id)`,
  идемпотентность. Отличие от `views`: оверлей — НЕ полноэкранная вью, а панель поверх тела со своей
  видимостью (`isOpen`-селектор) и своим Esc/close внутри компонента (взаимоисключаемость/стекинг —
  логика ui-стора, не реестра).
- **`isOpen: (state: UIState) => boolean`** — селектор из ui-стора (`(s) => s.goalsOpen`). F-8c читает
  СУЩЕСТВУЮЩИЕ `*Open`-були (перенос стейта в модули — вырезание F-10b, не здесь).
- **`OverlayOutlet` (`components/workspace/OverlayOutlet.tsx`)** — по образцу `MainViewOutlet`:
  рендерит `overlayRegistry.list()`, каждый открытый (`isOpen(uiState)`) — через per-contribution
  **ErrorBoundary** (`key` по id). Заменяет 7 хардкод-строк App.tsx. Счастливый путь — 0 DOM-след
  (якоря панелей не смещаются). Подписка на весь ui-стор (`useUIStore()`): `isOpen`-селекторы
  непрозрачны, outlet ре-рендерится на любое ui-изменение и заново считает видимость.
- **`core-overlays.ts`** — ядровые 7 оверлеев регистрируются напрямую (каркас, НЕ модуль — как
  `registerCoreViews`), сайд-эффектом при импорте из App.tsx. order 10..70 = прежний DOM-порядок
  App.tsx (стекинг floats digest/contradictions поверх trap-оверлеев сохранён).
- **v0-коупл (осознанно):** `isOpen` типизирован против `UIState` (ui-стор — источник флагов ядровых
  оверлеев; `UIState` экспортирован из `stores/ui.ts`, type-only импорт в `types.ts`). Store-agnostic
  абстракция состояния оверлея (для сторонних плагинов) отложена вместе с прочим north-star плагинов —
  как apiVersion/capabilities (см. «Отложено»).
- **Готово для F-10b:** `ctx.overlays.register(...)` в `ModuleContext`. Вырез оверлея в модуль =
  (1) панель через `ctx.overlays` в манифесте модуля, (2) убрать её из `core-overlays.ts`,
  (3) перенести `*Open`-стейт из ui-стора в модуль.

### titlebar-menu (AI-инсайты) — оставлено ядро-chrome (v0, F-8c)
AI-инсайты-меню Titlebar (пункты digest/goals/contradictions, `Titlebar.tsx` `aiItem`) **не вынесено в
реестр** — оставлено ядро-chrome для v0. Обоснование: (1) titlebar-menu-реестр = новый тип вклада +
реестр + поле `ModuleContext` + рефактор Titlebar + тесты — НЕ тривиален как overlays (те дословно
зеркалят `viewRegistry`); (2) **не нужен для разблокировки F-10b** — 7 вырезов требуют оверлей-реестра,
а пункт меню может остаться ядром, вызывая `openX()`/`toggleX()` ui-стора (стейт `*Open` уедет в модули
лишь в F-10b); (3) реестр «не нужный 7 оверлеям» = нарушение скоуп-дисциплины (YAGNI). Прецедент —
`DeadJobsModal` знает job-kind'ы (ядро-chrome-знание фич приемлемо в v0). Реестр titlebar-пунктов как
вклад модулей — возможная будущая EP (когда сторонний модуль захочет свой пункт AI-меню). → отложено.

### Реестр модулей
`modules.register(m)` (одно место) + `modules.activateAll()` — **детерминированный** порядок
активации (= порядок регистрации). `modules.disposeAll()` снимает все вклады всех модулей.
Единая точка регистрации прод-модулей — `connector/modules/index.ts` (`activateModules()`,
импортируется сайд-эффектом из `App.tsx`, как `core-views`). **В проде один модуль — `news`** (F-9);
F-10-серия добавляет свои строкой в `activateModules`. Тест-модуль-заглушка (`isolation.test.tsx`) —
для доказательства изоляции сбоев.

### ErrorBoundary per-contribution
`components/common/ErrorBoundary.tsx` — каждая зарегистрированная вью (`MainViewOutlet`), оверлей
(`OverlayOutlet`, F-8c) и секция настроек (`SettingsSectionOutlet`) оборачивается React-ErrorBoundary:
рендер-сбой вклада показывает плашку «модуль X упал» + reload вместо белого экрана. `CommandRegistry.run`
— в try/catch (упавшая команда → тост, не висящий reject). Цель владельца: **«ИИ правит модуль → app не
падает»**. Доказано `src/lib/connector/isolation.test.tsx` (падающая вью тест-модуля → app жив + плашка)
и `src/lib/connector/overlay-isolation.test.tsx` (падающий оверлей через `ctx.overlays` → app жив + плашка).

## Легализация (что было — что стало)
| Реестр    | Было (ядро)                              | Стало (через коннектор)                         |
|-----------|------------------------------------------|-------------------------------------------------|
| commands  | `commands-core` + `commands` registry    | `ctx.commands` — та же registry, id в namespace |
| views     | тернарник App.tsx + хардкод ActivityBar  | `viewRegistry` (core-views) → MainViewOutlet + ActivityBar |
| overlays  | 7 хардкод-строк `{xOpen && <Panel/>}` App.tsx | `overlayRegistry` (core-overlays) → OverlayOutlet (F-8c) |
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
