import { create } from 'zustand';

import type { GoalEntry } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';

/**
 * «Прогресс целей» (#35, vision-волна 2): vault-широкий список заметок-целей (инлайн-тег `#goal`) с
 * прогрессом из frontmatter `progress`. Чистый SQL-read (офлайн, без LLM). Живой пересчёт по любому
 * modify goal-файла требует event-канала индексатора (см. BACKLOG) — в v1 обновление по `load`
 * (открытие панели + кнопка «Обновить»).
 */
interface GoalsState {
  items: GoalEntry[];
  loading: boolean;
  load: () => Promise<void>;
}

export const useGoalsStore = create<GoalsState>((set) => ({
  items: [],
  loading: false,
  async load() {
    set({ loading: true });
    try {
      const items = await tauriApi.goals.list();
      set({ items, loading: false });
    } catch {
      set({ items: [], loading: false });
    }
  },
}));
