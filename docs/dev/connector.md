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
- **`isOpen: (state: UIState) => boolean`** — селектор из ui-стора (`(s) => s.goalsOpen`). Читает
  `*Open`-були (F-10b оставил их ядровыми — см. «ПАТТЕРН оверлей-модуля» ниже; модуль даёт селектор).
- **`OverlayOutlet` (`components/workspace/OverlayOutlet.tsx`)** — по образцу `MainViewOutlet`:
  рендерит `overlayRegistry.list()`, каждый открытый (`isOpen(uiState)`) — через per-contribution
  **ErrorBoundary** (`key` по id). Заменяет 7 хардкод-строк App.tsx. Счастливый путь — 0 DOM-след
  (якоря панелей не смещаются). Подписка на весь ui-стор (`useUIStore()`): `isOpen`-селекторы
  непрозрачны, outlet ре-рендерится на любое ui-изменение и заново считает видимость.
- **`core-overlays.ts`** — БЫЛ каркасом ядровых 7 оверлеев (как `registerCoreViews`). **F-10b удалил
  его**: все 7 вырезаны в модули (`connector/modules/*`), реестр `overlays` питается ТОЛЬКО модулями
  через `ctx.overlays`. order 10..70 (прежний DOM-порядок App.tsx, стекинг floats поверх trap) сохранён
  в манифестах модулей. См. «### F-10b: вырез оверлея в модуль» ниже.
- **v0-коупл (осознанно):** `isOpen` типизирован против `UIState` (ui-стор — источник флагов ядровых
  оверлеев; `UIState` экспортирован из `stores/ui.ts`, type-only импорт в `types.ts`). Store-agnostic
  абстракция состояния оверлея (для сторонних плагинов) отложена вместе с прочим north-star плагинов —
  как apiVersion/capabilities (см. «Отложено»).
### F-10b: вырез оверлея в модуль (ПАТТЕРН оверлей-модуля)

F-10b вырезал **все 7 оверлеев** (goals/memory/episodes/tasks/inbox/digest/contradictions) в модули
через `ctx.overlays` — `core-overlays.ts` удалён. **Behavior-preserving:** каждый оверлей
открывается/закрывается/стекается как раньше. Шаблон (`connector/modules/<feature>.ts`):

1. `ctx.overlays.register({ id, titleKey, isOpen, order, component })` — компонент панели + `isOpen`-
   селектор + order 10..70 перенесены КАК ЕСТЬ из `core-overlays.ts`.
2. `ctx.commands.register(...)` — команда палитры (у 6 из 7; у **episodes** команды НЕТ). id
   префиксуется модулем: `view.goals`→`goals:view.goals`, source=plugin. Хоткеи (⌘⇧K у tasks) +
   vault-guard'ы сохранены. Пара `view.X`→`X:view.X` в `COMMAND_ID_ALIASES` (`lib/commands.ts`) —
   ручной хоткей пользователя на старый id ремапится (иначе no-op).
3. `ctx.events.on(...)` — фича-эффект App.tsx рядом со своим оверлеем: `jobs:changed`-refetch у
   digest/contradictions (combined-эффект App.tsx расщеплён — каждая фича refetch'ит свой стор),
   `vault:changed`-дебаунс-пересчёт у goals.
4. Строка в `connector/modules/index.ts` (`modules.register(<feature>Module)`).

**ПАТТЕРН оверлей-модуля (v0, ГЛАВНОЕ решение F-10b): стейт видимости — ядровой, модуль даёт
компонент+isOpen.** Були `*Open` + действия `open/close/toggleX` + Esc-прецедент
(`selectReadingEscBlocked`) + trap-взаимоисключаемость (`TRAP_OVERLAYS_CLOSED`) **ОСТАЮТСЯ в ui-сторе
как ядро-управляемый стейт видимости оверлеев** (аналог `mainView`). Модуль лишь регистрирует `isOpen`-
СЕЛЕКТОР поверх ядрового флага + компонент + команду. Это чище store-agnostic-абстракции (YAGNI — до
сторонних плагинов): видимость/Esc/стекинг — ядро, компонент — модуль.

**grep-инвариант** «ядро (App/ActivityBar/Titlebar/SettingsView/OverlayOutlet) не импортирует
`components/<feature>`» **достигается переносом ИМПОРТА панели в манифест** (App.tsx/core-overlays её
больше не импортят — она в модуле). **`*Open`-стейт в ui-сторе — НЕ нарушение инварианта**
(стейт ≠ импорт компонента; ui-стор ядровой, управляет видимостью). Стереж на фичу:
`grep -rl "components/<feature>" src | grep -v 'components/<feature>/\|modules/<feature>.ts'` → пусто.

**Что оставлено ядро-chrome (обосновано — НЕ блокеры):** пункты меню «AI-инсайты» Titlebar
(goals/digest/contradictions) — titlebar-menu-реестра нет (см. ниже), зовут `toggleX()`; кнопки
«Память ИИ…»/«Эпизоды…» + тогглы agentMemory/episodic/insights/**contradictions** в ai-секции настроек
— тоггл фичи в ОБЩЕЙ ai-секции (НЕ отдельная секция настроек; `useAiFeaturesStore`/pref, не импорт
панели); кнопки «Задачи»/«Входящие» ActivityBar/«Сегодня»-вью — зовут `toggleX()` ui-стора. Все они
трогают ui/aiFeatures-стор, а НЕ `components/<feature>` → инвариант держится.

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
импортируется сайд-эффектом из `App.tsx`, как `core-views`). **В проде 8 модулей:** `news` (F-9) +
7 оверлей-модулей F-10b (goals/memory/episodes/tasks/inbox/digest/contradictions). Новый модуль —
строкой в `activateModules`. Тест-модуль-заглушка (`isolation.test.tsx`) — для изоляции сбоев.

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
| overlays  | 7 хардкод-строк `{xOpen && <Panel/>}` App.tsx | `overlayRegistry` → OverlayOutlet; питается 7 оверлей-модулями `ctx.overlays` (F-10b; core-overlays удалён) |
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

## Отложено в F-8b → частично сделано в F-10b
- **Миграция фича-эффектов App.tsx на `ctx.events`.** F-10b перевёл оверлей-эффекты на `ctx.events`
  вместе с вырезом их оверлеев: goals-reload (`vault:changed`) → `modules/goals.ts`,
  digest/contradictions-refetch (`jobs:changed`, combined-эффект расщеплён) → `modules/{digest,
  contradictions}.ts`. Остаток (episodic/aiFeatures-sync по смене vault) — НЕ оверлей-эффект, живёт в
  App.tsx (его фичи не вырезались); перевод на `onCoreEvent` behavior-preserving, отложен до их выреза.

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

**Инвариант** (с F-1b — стереж в CI, НЕ только grep-ом в ревью): единственный импортёр
`components/news` вне самой фичи — её манифест-модуль. Файл манифеста живёт вне `src/components/**`,
поэтому F-1 линт границ (запрет кросс-импортов между `components/<feature>`) его не трогает: модуль —
легальный слой проводки. **С F-1b тот же инвариант проверяет eslint** (не grep): импорт
`components/<mod>`/`modules/<mod>` из ядра/чужого модуля = красный CI. См. «Граница модуль/ядро в CI
(F-1b)» ниже.

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

## Граница модуль/ядро в CI (F-1b)
До F-1b инвариант «ядро не импортирует модуль» держался **grep-ом в adversarial-ревью** — CI его НЕ
проверял (F-10b-adversarial вскрыл: будущий импорт ядро→модуль CI бы пропустил, изоляция тихо
сломалась бы). F-1b **закрепил инвариант в eslint** (`apps/desktop/eslint.config.js`, блок F-1b —
рядом с F-1 «фича не импортирует фичу»).

**Что стережёт правило** (`MODULE_FEATURES` = список вырезанных модулей; сейчас 8:
`news, goals, memory, episodes, tasks, inbox, digest, contradictions`). Для каждого модуля есть ПАРА
изолированных зон: UI `src/components/<mod>/**` и манифест `src/lib/connector/modules/<mod>.ts`.
Запрещено импортировать эту пару откуда-либо, **КРОМЕ**:
- самой фичи (`components/<mod>/**` внутри себя — F-1 уже стережёт кросс-фичу);
- её манифеста `modules/<mod>.ts` (+ его теста `<mod>.test.ts`) — единственная легальная точка, где
  `components/<mod>` импортируется, и где живёт `activate(ctx)`.

Конкретно правило ловит то, что F-1 НЕ ловил:
- **ЯДРО** (всё вне `src/components/**` и вне `src/lib/connector/modules/**`: `App.tsx`, `stores/**`,
  `hooks/**`, `i18n/**`, `lib/**` вне connector) → импорт `components/<mod>` или конкретного манифеста
  `modules/<mod>` = **красный CI**. Барильер `lib/connector/modules` (активатор `activateModules`) —
  РАЗРЕШЁН (это композиционный корень, НЕ модуль).
- **Манифест `<X>.ts` → чужой манифест `<Y>.ts`** = красный (модули независимы, общаются через
  ядро/`ctx`; манифесты подключает ТОЛЬКО `modules/index.ts`).
- **`ctx`-API коннектора РАЗРЕШЁН ядру** (`lib/connector` — реестры/типы/`ModuleContext`/
  `module-manager`): правило запрещает только `modules/<mod>` и `components/<mod>`, НЕ connector-core.
- **FLOOR модуль-дир (adversarial F-1b):** ЛЮБОЙ файл в `src/lib/connector/modules/**`, кроме точных
  `<feature>.ts`/`<feature>.test.ts`/`index.ts` (стрэй-хелпер `news-helper.ts`; новый манифест
  `analytics.ts`, забытый в `MODULE_FEATURES`), закрыт floor-блоком: `selfModule:null` запрещает ВСЕ
  зоны/манифесты. Легит-файлы переопределяют floor блоками ниже. Без floor такой файл проваливался бы
  сквозь все блоки (0 правил) → laundering: манифест импортит легальный на вид `./news-helper`, а тот
  свободно тянет чужую зону в обход границы. Floor это закрывает.

Динамические `import()` покрыты компаньоном `no-restricted-syntax` (как F-1 §P2 — иначе границу обходил
бы `await import('../components/<mod>/…')`).

**Известное ограничение (низкий риск, честно):** импорт МАНИФЕСТА `modules/<mod>` из НЕ-модульной
компонент-зоны (напр. `components/agent` → `modules/news`) правило НЕ ловит — ядровой блок игнорирует
`src/components/**` (там правит F-1, чей `no-restricted-imports` нельзя дополнить без слияния правил
двух срезов). Импорт `components/<mod>` из компонент-зоны при этом ловится F-1 (кросс-фича), а из
ядра/манифеста — F-1b; непокрыт только «компонент-зона → чужой манифест». Риск низкий: манифесты не
тянут чужие манифесты (правило 3), а вырезанных компонент-зон, которым бы мог понадобиться чужой
манифест, нет — `agent`/`chat` ещё не модули. Закрытие потребовало бы влить F-1b-баны манифестов в
F-1-хелпер (смешение концернов двух срезов) — отложено до появления реального кейса.

**Исключения** (`MODULE_BOUNDARY_EXCEPTIONS` в eslint.config.js): пусто — F-9/F-10b вырезали модули
начисто. Если shared-компонент ЧЕСТНО нужен ядру — документируй файл там (аналог
`CROSS_IMPORT_WHITELIST` для F-1), НЕ ослабляй правило глобально.

**Negative-check (доказательство enforcement)** — `scripts/check-module-boundary.mjs` (гейт CI, job
frontend + `scripts/test-all.sh`): кладёт ВРЕМЕННЫЕ файлы с запрещённым импортом → eslint ОБЯЗАН упасть
(exit≠0). Два негатив-кейса: (1) ядровой файл → `components/news`+манифест; (2) laundering — стрэй-файл
ВНУТРИ `modules/` → чужая зона+манифест (доказывает, что floor закрыл дыру coverage). Плюс
позитив-контроль: реальные манифесты/тесты/`index` (легитимные self-импорты) остаются зелёными (нет
ложных срабатываний). Времянки всегда удаляются.

**Как добавить модуль F-10c в правило:** допиши его имя строкой в `MODULE_FEATURES`
(`apps/desktop/eslint.config.js`) — автоматически появятся и запрет его зоны/манифеста для ядра, и
разрешение для его манифеста+теста. Больше нигде правку делать не нужно.

## Как добавить модуль (общий рецепт)
1. Реализовать `NexusModule` (`activate(ctx)` регистрирует вклады через `ctx.*`) — см. эталон news.
2. `modules.register(myModule)` в `connector/modules/index.ts` (`activateAll` уже вызывается там).
3. **Добавить имя модуля в `MODULE_FEATURES`** (`apps/desktop/eslint.config.js`, F-1b) — так CI начнёт
   стеречь его границу (ядро/чужой модуль ⇏ его `components/<mod>`/манифест). Без этого шага новый
   модуль НЕ изолирован в CI.
4. Вклады автоматически изолированы ErrorBoundary; снятие — `modules.disposeAll()`.
