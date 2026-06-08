import { create } from 'zustand';

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
  toggleReading: () => set((s) => ({ reading: !s.reading })),
  closeReading: () => set({ reading: false }),
  toggleTweaks: () => set((s) => ({ tweaksOpen: !s.tweaksOpen })),
  closeTweaks: () => set({ tweaksOpen: false }),
  setSettingsSection: (settingsSection) => set({ settingsSection }),
  openSettings: (section = 'general') => set({ tweaksOpen: true, settingsSection: section }),
  setAiTab: (tab) => set({ aiTab: tab }),
}));
