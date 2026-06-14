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

type AiTab = 'chat' | 'suggest' | 'related';
/** Активная секция раздела настроек (Obsidian-style: левый нав → контент). Кросс-план #11. */
export type SettingsSection = 'general' | 'editor' | 'appearance' | 'ai' | 'hotkeys' | 'about';

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
  closeGoals: () => void;
  toggleGoals: () => void;
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
}

/** Глобальное UI-состояние оболочки (Command Palette, граф, RAG-чат и пр.). */
export const useUIStore = create<UIState>((set) => ({
  paletteOpen: false,
  graphOpen: false,
  chatOpen: false,
  pluginsOpen: false,
  syncOpen: false,
  conflictOpen: false,
  goalsOpen: false,
  tasksOpen: false,
  inboxOpen: false,
  digestOpen: false,
  contradictionsOpen: false,
  newsOpen: false,
  // HOME — стартовый лендинг после открытия vault (макет: Home-вью по умолчанию).
  homeOpen: true,
  onboardingDone: readOnboarded(),
  onboardingActive: false,
  tweaksOpen: false,
  settingsSection: 'general',
  sidebarOpen: true,
  reading: false,
  aiTab: 'chat',
  openPalette: () => set({ paletteOpen: true }),
  closePalette: () => set({ paletteOpen: false }),
  togglePalette: () => set((s) => ({ paletteOpen: !s.paletteOpen })),
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
    set({ chatOpen: true, homeOpen: false, newsOpen: false });
  },
  closeChat: () => set({ chatOpen: false }),
  toggleChat: () =>
    set((s) => {
      logUi('chat:toggle', s.chatOpen ? 'open→' : 'closed→');
      if (!s.chatOpen) return { chatOpen: true, homeOpen: false, newsOpen: false };
      // Панель уже «открыта», но скрыта за Home/News → клик возвращает её в поле зрения.
      if (s.homeOpen || s.newsOpen) return { homeOpen: false, newsOpen: false };
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
  // Модальные оверлеи goals/tasks/inbox взаимоисключаемы: открытие одного закрывает остальные —
  // иначе два focus-trap-диалога стекаются (клавиатурный капкан между ними, P9-ревью #5).
  closeGoals: () => set({ goalsOpen: false }),
  toggleGoals: () =>
    set((s) => {
      const open = !s.goalsOpen;
      logUi('goals:toggle', open ? 'open' : 'close');
      return open ? { goalsOpen: true, tasksOpen: false, inboxOpen: false } : { goalsOpen: false };
    }),
  closeTasks: () => set({ tasksOpen: false }),
  toggleTasks: () =>
    set((s) => {
      const open = !s.tasksOpen;
      logUi('tasks:toggle', open ? 'open' : 'close');
      return open ? { tasksOpen: true, goalsOpen: false, inboxOpen: false } : { tasksOpen: false };
    }),
  closeInbox: () => set({ inboxOpen: false }),
  toggleInbox: () =>
    set((s) => {
      const open = !s.inboxOpen;
      logUi('inbox:toggle', open ? 'open' : 'close');
      return open ? { inboxOpen: true, goalsOpen: false, tasksOpen: false } : { inboxOpen: false };
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
  // Полные вьюхи main-области взаимоисключающие: news ↔ home (редактор — когда обе закрыты).
  closeNews: () => set({ newsOpen: false }),
  toggleNews: () => set((s) => ({ newsOpen: !s.newsOpen, homeOpen: false })),
  openNews: () => {
    logUi('news:open');
    set({ newsOpen: true, homeOpen: false });
  },
  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  closeHome: () => set({ homeOpen: false }),
  toggleHome: () =>
    set((s) => {
      logUi('home:toggle', s.homeOpen ? 'close' : 'open');
      return { homeOpen: !s.homeOpen, newsOpen: false };
    }),
  openHome: () => set({ homeOpen: true, newsOpen: false }),
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
}));
