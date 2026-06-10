import { create } from 'zustand';

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
  closeGoals: () => void;
  toggleGoals: () => void;
  closeDigest: () => void;
  toggleDigest: () => void;
  closeContradictions: () => void;
  toggleContradictions: () => void;
  closeNews: () => void;
  toggleNews: () => void;
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
  goalsOpen: false,
  digestOpen: false,
  contradictionsOpen: false,
  newsOpen: false,
  // HOME — стартовый лендинг после открытия vault (макет: Home-вью по умолчанию).
  homeOpen: true,
  onboardingDone: readOnboarded(),
  onboardingActive: false,
  tweaksOpen: false,
  settingsSection: 'general',
  reading: false,
  aiTab: 'chat',
  openPalette: () => set({ paletteOpen: true }),
  closePalette: () => set({ paletteOpen: false }),
  togglePalette: () => set((s) => ({ paletteOpen: !s.paletteOpen })),
  openGraph: () => set({ graphOpen: true }),
  closeGraph: () => set({ graphOpen: false }),
  toggleGraph: () => set((s) => ({ graphOpen: !s.graphOpen })),
  openChat: () => set({ chatOpen: true }),
  closeChat: () => set({ chatOpen: false }),
  toggleChat: () => set((s) => ({ chatOpen: !s.chatOpen })),
  openPlugins: () => set({ pluginsOpen: true }),
  closePlugins: () => set({ pluginsOpen: false }),
  togglePlugins: () => set((s) => ({ pluginsOpen: !s.pluginsOpen })),
  closeSync: () => set({ syncOpen: false }),
  toggleSync: () => set((s) => ({ syncOpen: !s.syncOpen })),
  closeGoals: () => set({ goalsOpen: false }),
  toggleGoals: () => set((s) => ({ goalsOpen: !s.goalsOpen })),
  closeDigest: () => set({ digestOpen: false }),
  toggleDigest: () => set((s) => ({ digestOpen: !s.digestOpen })),
  closeContradictions: () => set({ contradictionsOpen: false }),
  toggleContradictions: () => set((s) => ({ contradictionsOpen: !s.contradictionsOpen })),
  // Полные вьюхи main-области взаимоисключающие: news ↔ home (редактор — когда обе закрыты).
  closeNews: () => set({ newsOpen: false }),
  toggleNews: () => set((s) => ({ newsOpen: !s.newsOpen, homeOpen: false })),
  closeHome: () => set({ homeOpen: false }),
  toggleHome: () => set((s) => ({ homeOpen: !s.homeOpen, newsOpen: false })),
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
  openSettings: (section = 'general') => set({ tweaksOpen: true, settingsSection: section }),
  setAiTab: (tab) => set({ aiTab: tab }),
}));
