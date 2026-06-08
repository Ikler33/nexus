import { create } from 'zustand';

import type { JobCounts } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';

/**
 * Счётчики очереди планировщика (ADR-007 срез 5) для индикатора в StatusBar. Обновляются по событию
 * `jobs:changed` (воркер шлёт после продуктивного тика) + при открытии vault. Чистый read, без поллинга.
 */
interface JobsState {
  counts: JobCounts;
  refresh: () => Promise<void>;
}

export const useJobsStore = create<JobsState>((set) => ({
  counts: { pending: 0, running: 0, dead: 0 },
  async refresh() {
    try {
      set({ counts: await tauriApi.scheduler.counts() });
    } catch {
      /* vault не открыт / нет данных — оставляем прежние */
    }
  },
}));
