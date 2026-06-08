import { create } from 'zustand';

import type { Contradiction } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';

/**
 * «Поиск противоречий» (#vision, спека `docs/specs/contradictions.md`): найденные пары конфликтующих
 * заметок. Поиск асинхронен (фоновая джоба планировщика): `generate()` ставит её в очередь, готовый
 * результат прилетает через `load()` по событию `jobs:changed` (см. App). `generating` снимается, когда
 * приходит набор с другим `createdAt`, чем был на момент клика (baseline).
 */
interface ContradictionsState {
  items: Contradiction[];
  loading: boolean;
  generating: boolean;
  error: string | null;
  /** `createdAt` первого элемента на момент клика «найти» — чтобы понять, что пришёл новый прогон. */
  baseline: number | null;
  load: () => Promise<void>;
  generate: () => Promise<void>;
}

const stamp = (items: Contradiction[]): number | null => items[0]?.createdAt ?? null;

export const useContradictionsStore = create<ContradictionsState>((set, get) => ({
  items: [],
  loading: false,
  generating: false,
  error: null,
  baseline: null,

  async load() {
    set({ loading: true });
    try {
      const items = await tauriApi.contradictions.list();
      const { generating, baseline } = get();
      const done = generating && stamp(items) !== baseline;
      set({ items, loading: false, ...(done ? { generating: false } : {}) });
    } catch {
      set({ loading: false });
    }
  },

  async generate() {
    set({ generating: true, error: null, baseline: stamp(get().items) });
    try {
      await tauriApi.contradictions.generate();
    } catch (e) {
      set({ generating: false, error: String(e) });
    }
  },
}));
