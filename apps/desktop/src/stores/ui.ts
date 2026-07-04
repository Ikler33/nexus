import { create } from 'zustand';

import { logUi } from '../lib/debug-log';

/** Флаг «онбординг пройден» (DP-7): welcome пропускает шаги настройки при повторных запусках. */
const ONBOARDED_KEY = 'nexus.onboarded.v1';

function readOnboarded(): boolean {
  try {
    return localStorage.getItem(ONBOARDED_KEY) === '1';
  } catch {
    return false;
  }
}

type AiTab = 'chat' | 'agent';

/**
 * F-4 — ЧЕТЫРЕ семейства UI-состояния этого стора (thermo-смелл №1: булев-взрыв ~25 флагов).
 * Границы реестров зафиксированы, чтобы новые вью/оверлеи не «слипались»:
 *
 *  1) mainView    — полноэкранные ВЗАИМОИСКЛЮЧАЕМЫЕ вьюхи (home/today/news/board/agent, редактор —
 *     когда все закрыты). Приоритет — тернарник App.tsx (agent > today > home > news > board > editor).
 *     Единый источник ЧТЕНИЯ активной вью — `selectMainView`.
 *  2) trapOverlay — focus-trap top-оверлеи (палитра/шпаргалка/Цели/Задачи/Входящие/Память/Эпизоды/
 *     Настройки): открытие одного гасит остальные (TRAP_OVERLAYS_CLOSED, урок P9-ревью #5). В DOM
 *     рендерятся НЕЗАВИСИМО (не через один слот) — `selectTrapOverlay` только ЧИТАЕТ «какой сверху».
 *     Схлопывание булей в один стейт отложено (F-4b): tweaks-дрейф + независимый рендер.
 *  3) floats      — независимые плавающие слои (граф/плагины/sync/дайджест/противоречия/чат/сайдбар/
 *     reading/aiTab): НЕ взаимоисключаемы, живут отдельными булями (F-4b, если вообще).
 *  4) safe-flow   — модальные потоки редактора (conflict/versions/capture/templates): закрываются
 *     ЯВНО, при nav-переходе (SWITCH_MAIN) НЕ гасятся. Отдельный реестр — НЕ folded в trap.
 */
export type MainView = 'home' | 'today' | 'news' | 'board' | 'agent' | 'editor';
export type TrapOverlay =
  | 'palette'
  | 'cheatsheet'
  | 'goals'
  | 'tasks'
  | 'inbox'
  | 'memory'
  | 'episodes'
  | 'settings';

/** Активная секция раздела настроек (Obsidian-style: левый нав → контент). Кросс-план #11. */
export type SettingsSection =
  | 'general'
  | 'editor'
  | 'appearance'
  | 'ai'
  | 'news'
  | 'data'
  | 'hotkeys'
  | 'about';

interface UIState {
  paletteOpen: boolean;
  graphOpen: boolean;
  chatOpen: boolean;
  /** Открыта ли панель плагинов (sandbox-iframe, Ф2). */
  pluginsOpen: boolean;
  /** Открыта ли панель синхронизации (git-sync, Ф3). */
  syncOpen: boolean;
  /** Открыта ли панель «Цели» (#35, vision). */
  goalsOpen: boolean;
  /** Открыта ли панель «Задачи» (TASK-1 — сводка всех `- [ ]` vault). */
  tasksOpen: boolean;
  /** Открыта ли панель «Входящие» (INBOX-1 — GTD-разбор Inbox.md). */
  inboxOpen: boolean;
  /** Открыта ли панель «Дайджест изменений» (#35, ADR-007 slice 4). */
  digestOpen: boolean;
  /** Открыта ли панель «Поиск противоречий» (#vision). */
  contradictionsOpen: boolean;
  /** Открыта ли страница «Новости» (NF-5) — полная вью вместо редактора. */
  newsOpen: boolean;
  /** Открыта ли «Доска» (BOARD-4) — канбан-вью заметок-задач вместо редактора. */
  boardOpen: boolean;
  /** Открыт ли утренний экран «Сегодня» (TODAY-1) — сводка дня вместо редактора. */
  todayOpen: boolean;
  /** Открыта ли вкладка «Агент» (UI-1) — full-screen агентский воркспейс вместо редактора. */
  agentOpen: boolean;
  /** Открыт ли HOME-дашборд (DP-1) — лендинг-вью вместо редактора (стартовая после vault). */
  homeOpen: boolean;
  /** Онбординг пройден (DP-7, персист): welcome ведёт сразу к открытию vault. */
  onboardingDone: boolean;
  /** Многошаговый онбординг идёт прямо сейчас (держит экран и после открытия vault). */
  onboardingActive: boolean;
  /** Открыт ли раздел настроек (модалка Obsidian-style; `tweaksOpen` исторически — теперь весь раздел). */
  tweaksOpen: boolean;
  /** Активная секция раздела настроек. */
  settingsSection: SettingsSection;
  /** Видимость сайдбара (DP-13: кнопка «Файлы» activity-bar сворачивает панель, как в макете). */
  sidebarOpen: boolean;
  /** Режим чтения (⌘R): прячет сайдбар/AI-панель, центрирует документ (distraction-free). */
  reading: boolean;
  /** Активная вкладка AI-панели (чат / связи). */
  aiTab: AiTab;
  openPalette: () => void;
  closePalette: () => void;
  togglePalette: () => void;
  openGraph: () => void;
  closeGraph: () => void;
  toggleGraph: () => void;
  openChat: () => void;
  closeChat: () => void;
  toggleChat: () => void;
  openPlugins: () => void;
  closePlugins: () => void;
  togglePlugins: () => void;
  closeSync: () => void;
  toggleSync: () => void;
  /** Конфликт-резолвер из пилюли статусбара (DP-14: открывается напрямую, как onConflict макета). */
  conflictOpen: boolean;
  openConflict: () => void;
  closeConflict: () => void;
  /** История версий активной заметки (SAFE-6: список снапшотов + diff + восстановление). */
  versionsOpen: boolean;
  openVersions: () => void;
  closeVersions: () => void;
  /** Quick-capture: мини-модалка мгновенной записи мысли в Inbox (CAP-2, ⌘⇧N). */
  captureOpen: boolean;
  openCapture: () => void;
  closeCapture: () => void;
  /** Выбор шаблона: модалка «Новая заметка из шаблона» (CAP-3, ⌘⇧T). */
  templatesOpen: boolean;
  openTemplates: () => void;
  closeTemplates: () => void;
  /** Шпаргалка горячих клавиш (POLISH, ⌘/): overlay со списком сочетаний из реестра команд. */
  cheatsheetOpen: boolean;
  openCheatsheet: () => void;
  closeCheatsheet: () => void;
  toggleCheatsheet: () => void;
  closeGoals: () => void;
  toggleGoals: () => void;
  /** Открыта ли панель «Память ИИ» (MEM-4 — явные факты памяти агента). */
  memoryOpen: boolean;
  closeMemory: () => void;
  toggleMemory: () => void;
  /** Открыть «Память ИИ» (из Настроек — поэтому закрывает раздел настроек). */
  openMemory: () => void;
  /** Открыта ли панель «Эпизоды» (EP-3 — саммари прошлых сессий). */
  episodesOpen: boolean;
  closeEpisodes: () => void;
  toggleEpisodes: () => void;
  /** Открыть «Эпизоды» (из Настроек — закрывает раздел настроек). */
  openEpisodes: () => void;
  closeTasks: () => void;
  toggleTasks: () => void;
  closeInbox: () => void;
  toggleInbox: () => void;
  closeDigest: () => void;
  toggleDigest: () => void;
  closeContradictions: () => void;
  toggleContradictions: () => void;
  closeNews: () => void;
  toggleNews: () => void;
  /** Открыть «Новости» (activity-bar: клик = переход на вью, не тоггл — как setView макета). */
  openNews: () => void;
  closeBoard: () => void;
  toggleBoard: () => void;
  /** Открыть «Доску» (activity-bar: клик = переход на вью, гасит home/news/chat). */
  openBoard: () => void;
  closeToday: () => void;
  toggleToday: () => void;
  /** Открыть «Сегодня» (activity-bar: клик = переход на вью, гасит home/news/board/chat). */
  openToday: () => void;
  closeAgent: () => void;
  toggleAgent: () => void;
  /** Открыть «Агент» (activity-bar: клик = переход на вью, гасит home/news/board/today/chat). При
   *  передаче `seed` — предзаполнить композер агента этим текстом (Castor «Быстрый старт»: каждый
   *  пункт сидит свой промпт, НЕ авто-отправляя — пользователь жмёт «Запустить» сам). */
  openAgent: (seed?: string) => void;
  /** AGENT-SEED: «отложенный» промпт для композера агента — `openAgent(seed)` кладёт сюда, AgentView
   *  читает в поле ввода и сбрасывает (consumeAgentSeed). `seq` (как revealTarget) — чтобы ПОВТОРНЫЙ
   *  сид того же текста перезаписывал поле (иначе один и тот же промпт второй раз не «пролил» бы). */
  pendingAgentSeed: { text: string; seq: number } | null;
  /** Сбросить отложенный промпт агента (AgentView вызывает после prefill композера). */
  consumeAgentSeed: () => void;
  toggleSidebar: () => void;
  closeHome: () => void;
  toggleHome: () => void;
  openHome: () => void;
  startOnboarding: () => void;
  finishOnboarding: () => void;
  toggleReading: () => void;
  closeReading: () => void;
  toggleTweaks: () => void;
  closeTweaks: () => void;
  setSettingsSection: (section: SettingsSection) => void;
  /** Открыть раздел настроек сразу на нужной секции. */
  openSettings: (section?: SettingsSection) => void;
  setAiTab: (tab: AiTab) => void;
  /** TAGCLICK-1: «отложенный» тег-фильтр — клик по `#tag`-чипу в превью просит сайдбар открыть панель
   *  поиска с ТОЧНЫМ фильтром по тегу. Сайдбар читает значение и сбрасывает его (consumeTagFilter). */
  pendingTagFilter: string | null;
  /** Запросить фильтр сайдбара по тегу (показывает сайдбар, выходит из reading-режима). */
  openTagFilter: (tag: string) => void;
  /** Сбросить отложенный тег-фильтр (сайдбар вызывает после применения). */
  consumeTagFilter: () => void;
  /** Связи в инспекторе (Hermes-6: AI-панель = Чат+Castor, «Связи» переехали в инспектор-рейл
   *  редактора). Команда палитры «Связи» просит редактор открыть секцию; InspectorRail читает и
   *  сбрасывает (consumeInspectorSection), как pendingTagFilter. */
  pendingInspectorSection: string | null;
  openInspectorSection: (section: string) => void;
  consumeInspectorSection: () => void;
  /** REVEAL-ACTIVE-FILE: запрос «показать файл в дереве» — `seq` (а не голый путь), чтобы повтор по
   *  ТОМУ ЖЕ пути перезапускал эффект скролла. FileTree подписан, скроллит и сбрасывает. */
  revealTarget: { path: string; seq: number } | null;
  /** Запросить показ файла в дереве (открывает сайдбар, выходит из reading). */
  requestReveal: (path: string) => void;
  /** Сбросить запрос показа (FileTree вызывает после скролла). */
  consumeReveal: () => void;
  /** FILE-RENAME-COMMAND: запрос «переименовать файл в дереве» — `seq` для перезапуска по тому же
   *  пути. FileTree подписан: скроллит, открывает инлайн-input, сбрасывает. */
  renameTarget: { path: string; seq: number } | null;
  /** Запросить инлайн-переименование файла в дереве (открывает сайдбар, выходит из reading). */
  requestRename: (path: string) => void;
  /** Сбросить запрос переименования (FileTree вызывает после открытия input). */
  consumeRename: () => void;
}

/**
 * Top-оверлеи с focus-trap/верхним z (палитра, шпаргалка, Goals/Tasks/Inbox): открытие ОДНОГО гасит
 * остальные. Иначе два focus-trap-диалога стекаются и дают клавиатурный капкан (урок P9-ревью #5 +
 * adversarial-ревью шпаргатки: ⌘/ поверх открытой панели). Спред этой константы в open-ветках.
 */
const TRAP_OVERLAYS_CLOSED = {
  paletteOpen: false,
  goalsOpen: false,
  tasksOpen: false,
  inboxOpen: false,
  cheatsheetOpen: false,
  memoryOpen: false,
  episodesOpen: false,
  tweaksOpen: false, // ревью MEM-4: иначе trap-оверлей поверх открытых Настроек = два стэкнутых focus-trap
} as const;

/**
 * Полноэкранные main-вьюхи взаимоисключаемы (home ↔ news ↔ board ↔ today ↔ agent; редактор — когда
 * все закрыты). Спред этой константы в каждой open/toggle-ветке гасит остальные одним местом — иначе
 * при добавлении новой вью (UI-1 «Агент») легко забыть один из переходов и две вью наложатся.
 */
const MAIN_VIEWS_CLOSED = {
  homeOpen: false,
  newsOpen: false,
  boardOpen: false,
  todayOpen: false,
  agentOpen: false,
} as const;

/**
 * W-6: переход на полноэкранную main-вью (Home/News/Board/Today/Agent) обязан ПОГАСИТЬ и плавающие
 * слои, которые иначе остаются ПОВЕРХ main-области и навигация «не срабатывает» из них (ST-D1: граф —
 * absolute-слой; Tasks/Inbox/Sync/Plugins/Goals/Memory/Episodes/палитра/шпаргалка/Настройки — top-
 * overlays в App.tsx). Не трогаем conflict/versions (модальные safe-flow редактора — закрываются явно).
 */
const SWITCH_MAIN = {
  ...MAIN_VIEWS_CLOSED,
  ...TRAP_OVERLAYS_CLOSED, // palette/goals/tasks/inbox/cheatsheet/memory/episodes/tweaks
  graphOpen: false,
  pluginsOpen: false,
  syncOpen: false,
  digestOpen: false,
  contradictionsOpen: false,
} as const;

/**
 * F-4 — DERIVED-селектор активной main-вью (семейство 1). Читает текущие `*Open`-були В ПРИОРИТЕТЕ
 * тернарника App.tsx (agent > today > home > news > board > editor). Единственный правильный способ
 * спросить «какая вью сейчас»: потребители подписываются `useUIStore(selectMainView)` (примитив →
 * React бэйлит ре-рендер). Коннектор F-8 строит реестр вью поверх этого enum.
 */
export const selectMainView = (s: UIState): MainView => {
  if (s.agentOpen) return 'agent';
  if (s.todayOpen) return 'today';
  if (s.homeOpen) return 'home';
  if (s.newsOpen) return 'news';
  if (s.boardOpen) return 'board';
  return 'editor';
};

/**
 * F-4 — DERIVED-селектор верхнего trap-оверлея (семейство 2): какой focus-trap-оверлей активен, или
 * null. Реестр взаимоисключаем по построению (TRAP_OVERLAYS_CLOSED), но tweaks-дрейф (toggleTweaks/
 * openSettings НЕ спредят реестр) допускает Настройки поверх другого — приоритет детерминирует «кто
 * сверху». Только для ЧТЕНИЯ: рендер оверлеев в App.tsx остаётся независимым (поведение то же).
 */
export const selectTrapOverlay = (s: UIState): TrapOverlay | null => {
  if (s.paletteOpen) return 'palette';
  if (s.cheatsheetOpen) return 'cheatsheet';
  if (s.goalsOpen) return 'goals';
  if (s.tasksOpen) return 'tasks';
  if (s.inboxOpen) return 'inbox';
  if (s.memoryOpen) return 'memory';
  if (s.episodesOpen) return 'episodes';
  if (s.tweaksOpen) return 'settings';
  return null;
};

/**
 * F-4 — Esc-прецедент reading-режима (семейства 2+3+4), кодифицирован КАК ЕСТЬ (App.tsx reading-Esc-
 * гейт). Любой оверлей ПОВЕРХ reading перехватывает Esc (у него свой close) → глобальный Esc App.tsx
 * НЕ выходит из чтения «сквозь» модалку (аудит reading-esc-precedence + conflictresolver-esc). Union
 * ВСЕХ трёх реестров-модалок (trap + блокирующие floats + safe-flow); НЕ включает main-вью/chat/
 * reading/sidebar. P0-3-смоук (overlays.spec «Режим чтения…») пинит это поведение.
 */
export const selectReadingEscBlocked = (s: UIState): boolean =>
  s.paletteOpen ||
  s.graphOpen ||
  s.pluginsOpen ||
  s.syncOpen ||
  s.captureOpen ||
  s.templatesOpen ||
  s.versionsOpen ||
  s.cheatsheetOpen ||
  s.conflictOpen ||
  s.goalsOpen ||
  s.memoryOpen ||
  s.episodesOpen ||
  s.tasksOpen ||
  s.inboxOpen ||
  s.digestOpen ||
  s.contradictionsOpen ||
  s.tweaksOpen;

/** Глобальное UI-состояние оболочки (Command Palette, граф, RAG-чат и пр.). */
export const useUIStore = create<UIState>((set) => ({
  paletteOpen: false,
  graphOpen: false,
  chatOpen: false,
  pluginsOpen: false,
  syncOpen: false,
  conflictOpen: false,
  goalsOpen: false,
  memoryOpen: false,
  episodesOpen: false,
  tasksOpen: false,
  inboxOpen: false,
  digestOpen: false,
  contradictionsOpen: false,
  newsOpen: false,
  boardOpen: false,
  todayOpen: false,
  agentOpen: false,
  // HOME — стартовый лендинг после открытия vault (макет: Home-вью по умолчанию).
  homeOpen: true,
  onboardingDone: readOnboarded(),
  onboardingActive: false,
  tweaksOpen: false,
  settingsSection: 'general',
  sidebarOpen: true,
  reading: false,
  aiTab: 'chat',
  openPalette: () => set({ ...TRAP_OVERLAYS_CLOSED, paletteOpen: true }),
  closePalette: () => set({ paletteOpen: false }),
  togglePalette: () =>
    set((s) => (s.paletteOpen ? { paletteOpen: false } : { ...TRAP_OVERLAYS_CLOSED, paletteOpen: true })),
  openGraph: () => set({ graphOpen: true }),
  closeGraph: () => set({ graphOpen: false }),
  toggleGraph: () =>
    set((s) => {
      logUi('graph:toggle', s.graphOpen ? 'close' : 'open');
      return { graphOpen: !s.graphOpen };
    }),
  // AI-панель живёт только в workspace-вью (DP-12, макет) → открытие чата с Home/News обязано
  // выводить в workspace, иначе флаг взводится, а панель не видна — «мёртвая кнопка» (баг
  // владельца 2026-06-11: приложение стартует на Home, и чат «не открывался»).
  openChat: () => {
    logUi('chat:open');
    // W-6: SWITCH_MAIN (а не только MAIN_VIEWS_CLOSED) — иначе панель «открыта», но скрыта под графом/
    // оверлеем (тот же ST-D1, что у main-нав). Чат — workspace-панель, гасим блокирующие слои.
    set({ ...SWITCH_MAIN, chatOpen: true });
  },
  closeChat: () => set({ chatOpen: false }),
  toggleChat: () =>
    set((s) => {
      logUi('chat:toggle', s.chatOpen ? 'open→' : 'closed→');
      if (!s.chatOpen) return { ...SWITCH_MAIN, chatOpen: true };
      // Панель уже «открыта», но скрыта за main-вью ИЛИ плавающим/trap-слоем (граф/Tasks/Inbox/Goals/
      // Memory/Episodes/Digest/Contradictions/Sync/…) → клик возвращает её в поле зрения (W-6).
      // B2: набор блокирующих слоёв = РОВНО ключи SWITCH_MAIN (что ветка гасит — то и проверяет,
      // список не дрейфует). Раньше рукописный список терял goals/memory/episodes/digest/
      // contradictions, и чат «открывался» ПОД ними.
      const blocked = (Object.keys(SWITCH_MAIN) as (keyof typeof SWITCH_MAIN)[]).some((k) => s[k]);
      if (blocked) return { ...SWITCH_MAIN };
      return { chatOpen: false };
    }),
  openPlugins: () => set({ pluginsOpen: true }),
  closePlugins: () => set({ pluginsOpen: false }),
  togglePlugins: () => set((s) => ({ pluginsOpen: !s.pluginsOpen })),
  closeSync: () => set({ syncOpen: false }),
  toggleSync: () => set((s) => ({ syncOpen: !s.syncOpen })),
  openConflict: () => set({ conflictOpen: true }),
  closeConflict: () => set({ conflictOpen: false }),
  versionsOpen: false,
  openVersions: () => set({ versionsOpen: true }),
  closeVersions: () => set({ versionsOpen: false }),
  captureOpen: false,
  openCapture: () => set({ captureOpen: true }),
  closeCapture: () => set({ captureOpen: false }),
  templatesOpen: false,
  openTemplates: () => set({ templatesOpen: true }),
  closeTemplates: () => set({ templatesOpen: false }),
  cheatsheetOpen: false,
  openCheatsheet: () => set({ ...TRAP_OVERLAYS_CLOSED, cheatsheetOpen: true }),
  closeCheatsheet: () => set({ cheatsheetOpen: false }),
  toggleCheatsheet: () =>
    set((s) => {
      const open = !s.cheatsheetOpen;
      logUi('cheatsheet:toggle', open ? 'open' : 'close');
      return open ? { ...TRAP_OVERLAYS_CLOSED, cheatsheetOpen: true } : { cheatsheetOpen: false };
    }),
  // Модальные оверлеи goals/tasks/inbox взаимоисключаемы: открытие одного закрывает остальные —
  // иначе два focus-trap-диалога стекаются (клавиатурный капкан между ними, P9-ревью #5).
  closeGoals: () => set({ goalsOpen: false }),
  toggleGoals: () =>
    set((s) => {
      const open = !s.goalsOpen;
      logUi('goals:toggle', open ? 'open' : 'close');
      return open ? { ...TRAP_OVERLAYS_CLOSED, goalsOpen: true } : { goalsOpen: false };
    }),
  // «Память ИИ» (MEM-4) — focus-trap-модалка, взаимоисключаема с прочими trap-оверлеями (включая Настройки
  // `tweaksOpen` — теперь в TRAP_OVERLAYS_CLOSED, чтобы НИ ОДИН trap-оверлей не стэкался поверх Настроек).
  closeMemory: () => set({ memoryOpen: false }),
  toggleMemory: () =>
    set((s) => {
      const open = !s.memoryOpen;
      logUi('memory:toggle', open ? 'open' : 'close');
      return open
        ? { ...TRAP_OVERLAYS_CLOSED, memoryOpen: true, tweaksOpen: false }
        : { memoryOpen: false };
    }),
  // Открытие из Настроек: закрываем раздел настроек, чтобы модалка не пряталась под ним.
  openMemory: () => set({ ...TRAP_OVERLAYS_CLOSED, memoryOpen: true, tweaksOpen: false }),
  // «Эпизоды» (EP-3) — focus-trap-модалка, взаимоисключаема с прочими trap-оверлеями (как «Память ИИ»).
  closeEpisodes: () => set({ episodesOpen: false }),
  toggleEpisodes: () =>
    set((s) => {
      const open = !s.episodesOpen;
      logUi('episodes:toggle', open ? 'open' : 'close');
      return open
        ? { ...TRAP_OVERLAYS_CLOSED, episodesOpen: true, tweaksOpen: false }
        : { episodesOpen: false };
    }),
  openEpisodes: () => set({ ...TRAP_OVERLAYS_CLOSED, episodesOpen: true, tweaksOpen: false }),
  closeTasks: () => set({ tasksOpen: false }),
  toggleTasks: () =>
    set((s) => {
      const open = !s.tasksOpen;
      logUi('tasks:toggle', open ? 'open' : 'close');
      return open ? { ...TRAP_OVERLAYS_CLOSED, tasksOpen: true } : { tasksOpen: false };
    }),
  closeInbox: () => set({ inboxOpen: false }),
  toggleInbox: () =>
    set((s) => {
      const open = !s.inboxOpen;
      logUi('inbox:toggle', open ? 'open' : 'close');
      return open ? { ...TRAP_OVERLAYS_CLOSED, inboxOpen: true } : { inboxOpen: false };
    }),
  closeDigest: () => set({ digestOpen: false }),
  toggleDigest: () =>
    set((s) => {
      logUi('digest:toggle', s.digestOpen ? 'close' : 'open');
      return { digestOpen: !s.digestOpen };
    }),
  closeContradictions: () => set({ contradictionsOpen: false }),
  toggleContradictions: () =>
    set((s) => {
      logUi('contradictions:toggle', s.contradictionsOpen ? 'close' : 'open');
      return { contradictionsOpen: !s.contradictionsOpen };
    }),
  // Полные вьюхи main-области взаимоисключающие: news ↔ home ↔ board ↔ today ↔ agent (редактор — когда все закрыты).
  closeNews: () => set({ newsOpen: false }),
  toggleNews: () => set((s) => ({ ...SWITCH_MAIN, newsOpen: !s.newsOpen })),
  openNews: () => {
    logUi('news:open');
    set({ ...SWITCH_MAIN, newsOpen: true });
  },
  closeBoard: () => set({ boardOpen: false }),
  toggleBoard: () =>
    set((s) => {
      logUi('board:toggle', s.boardOpen ? 'close' : 'open');
      return { ...SWITCH_MAIN, boardOpen: !s.boardOpen };
    }),
  openBoard: () => {
    logUi('board:open');
    set({ ...SWITCH_MAIN, boardOpen: true });
  },
  // «Сегодня» (TODAY-1) — полная main-вью, взаимоисключаема с home/news/board/agent (как они меж собой).
  closeToday: () => set({ todayOpen: false }),
  toggleToday: () =>
    set((s) => {
      logUi('today:toggle', s.todayOpen ? 'close' : 'open');
      return { ...SWITCH_MAIN, todayOpen: !s.todayOpen };
    }),
  openToday: () => {
    logUi('today:open');
    set({ ...SWITCH_MAIN, todayOpen: true });
  },
  // «Агент» (UI-1) — полная main-вью, взаимоисключаема с home/news/board/today (как они меж собой).
  closeAgent: () => set({ agentOpen: false }),
  toggleAgent: () =>
    set((s) => {
      logUi('agent:toggle', s.agentOpen ? 'close' : 'open');
      return { ...SWITCH_MAIN, agentOpen: !s.agentOpen };
    }),
  openAgent: (seed?: string) => {
    logUi('agent:open', seed ? 'seeded' : '');
    // Сид пишем как { text, seq } (как revealTarget): повтор того же промпта растит seq → AgentView
    // перезапускает prefill. Пустой/пробельный seed (или без него) — поле не трогаем.
    set((s) => ({
      ...SWITCH_MAIN,
      agentOpen: true,
      pendingAgentSeed:
        seed && seed.trim()
          ? { text: seed, seq: (s.pendingAgentSeed?.seq ?? 0) + 1 }
          : s.pendingAgentSeed,
    }));
  },
  pendingAgentSeed: null,
  consumeAgentSeed: () => set({ pendingAgentSeed: null }),
  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  closeHome: () => set({ homeOpen: false }),
  toggleHome: () =>
    set((s) => {
      logUi('home:toggle', s.homeOpen ? 'close' : 'open');
      return { ...SWITCH_MAIN, homeOpen: !s.homeOpen };
    }),
  openHome: () => set({ ...SWITCH_MAIN, homeOpen: true }),
  startOnboarding: () => set({ onboardingActive: true }),
  finishOnboarding: () => {
    try {
      localStorage.setItem(ONBOARDED_KEY, '1');
    } catch {
      /* ignore */
    }
    set({ onboardingDone: true, onboardingActive: false });
  },
  toggleReading: () => set((s) => ({ reading: !s.reading })),
  closeReading: () => set({ reading: false }),
  toggleTweaks: () => set((s) => ({ tweaksOpen: !s.tweaksOpen })),
  closeTweaks: () => set({ tweaksOpen: false }),
  setSettingsSection: (settingsSection) => set({ settingsSection }),
  openSettings: (section = 'general') => {
    logUi('settings:open', section);
    set({ tweaksOpen: true, settingsSection: section });
  },
  setAiTab: (tab) => set({ aiTab: tab }),
  pendingTagFilter: null,
  // Показать сайдбар и выйти из reading (там сайдбар скрыт), иначе фильтр применится незаметно.
  openTagFilter: (tag) => set({ pendingTagFilter: tag, sidebarOpen: true, reading: false }),
  consumeTagFilter: () => set({ pendingTagFilter: null }),
  pendingInspectorSection: null,
  // Закрываем оверлейные вью + выходим из reading (там инспектор-рейл скрыт, как у openTagFilter),
  // чтобы показался редактор с рейлом, и просим открыть секцию.
  openInspectorSection: (section) =>
    // W-6: ведёт в редактор+inspector-rail → гасим граф/оверлеи (SWITCH_MAIN), иначе цель скрыта под ними.
    set({ ...SWITCH_MAIN, reading: false, pendingInspectorSection: section }),
  consumeInspectorSection: () => set({ pendingInspectorSection: null }),
  revealTarget: null,
  // Дерево видно только при открытом сайдбаре и не в reading — иначе скролл произойдёт незаметно.
  requestReveal: (path) =>
    set((s) => ({
      revealTarget: { path, seq: (s.revealTarget?.seq ?? 0) + 1 },
      sidebarOpen: true,
      reading: false,
    })),
  consumeReveal: () => set({ revealTarget: null }),
  renameTarget: null,
  requestRename: (path) =>
    set((s) => ({
      renameTarget: { path, seq: (s.renameTarget?.seq ?? 0) + 1 },
      sidebarOpen: true,
      reading: false,
    })),
  consumeRename: () => set({ renameTarget: null }),
}));
