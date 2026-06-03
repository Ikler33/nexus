import { create } from 'zustand';

type AiTab = 'chat' | 'suggest';

interface UIState {
  paletteOpen: boolean;
  graphOpen: boolean;
  chatOpen: boolean;
  /** Открыта ли панель плагинов (sandbox-iframe, Ф2). */
  pluginsOpen: boolean;
  /** Открыта ли панель синхронизации (git-sync, Ф3). */
  syncOpen: boolean;
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
  setAiTab: (tab: AiTab) => void;
}

/** Глобальное UI-состояние оболочки (Command Palette, граф, RAG-чат и пр.). */
export const useUIStore = create<UIState>((set) => ({
  paletteOpen: false,
  graphOpen: false,
  chatOpen: false,
  pluginsOpen: false,
  syncOpen: false,
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
  setAiTab: (tab) => set({ aiTab: tab }),
}));
