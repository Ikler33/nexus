import { create } from 'zustand';

import type { Contradiction } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';
import { isJobReady } from '../lib/jobs';

/**
 * «Поиск противоречий» (#vision, спека `docs/specs/contradictions.md`): найденные пары конфликтующих
 * заметок. Поиск асинхронен (фоновая джоба планировщика): `generate()` ставит её в очередь, готовый
 * результат прилетает через `load()` по событию `jobs:changed` (см. App). `generating` снимается, когда
 * приходит набор с другим `createdAt`, чем был на момент клика (baseline).
 *
 * ⚠️ NB-4: `contradictions` — recurring-kind (раз/сутки). После завершения прогона воркер НЕМЕДЛЕННО
 * ставит следующий `pending` «на завтра» (reschedule_if_absent). Поэтому `jobActive('contradictions')`
 * (Rust `is_kind_busy`) в steady state всегда возвращал `true` → вечный «Ищу…» при сбое.
 * Фикс: `isJobReady` (ready-семантика, зеркало Rust `has_ready_job`) — только running/pending
 * с наступившим run_at считается текущим прогоном.
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
      let stillGenerating = generating;
      if (generating) {
        const gotNew = stamp(items) !== baseline;
        // Завершилось: новый прогон ИЛИ нет ГОТОВОЙ джобы (running/pending с наступившим run_at).
        // NB-4: НЕ jobActive — он считает и «завтрашнюю» recurring-pending «занятой» → вечный спиннер.
        const activeList = await tauriApi.scheduler.activeJobs();
        if (gotNew || !isJobReady('contradictions', activeList, Date.now())) {
          stillGenerating = false;
        }
      }
      set({ items, loading: false, generating: stillGenerating });
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
