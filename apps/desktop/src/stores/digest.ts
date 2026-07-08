import { create } from 'zustand';

import type { Digest } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';
import { isJobReady } from '../lib/jobs';

/**
 * «Дайджест изменений» (#35, ADR-007 slice 4): последний LLM-дайджест недавно изменённых заметок.
 * Генерация асинхронна (джоба планировщика): `generate()` ставит её в очередь, а готовый результат
 * прилетает через `load()` по событию `jobs:changed` (см. App). `generating` снимается, когда из БД
 * приходит дайджест свежее того, что был на момент клика (baseline).
 *
 * ⚠️ NB-4: `digest` — recurring-kind (раз/сутки). После завершения прогона воркер НЕМЕДЛЕННО
 * ставит следующий `pending` «на завтра» (reschedule_if_absent). Поэтому `jobActive('digest')`
 * (Rust `is_kind_busy`) в steady state всегда возвращал `true` → вечный «Генерирую…» при сбое.
 * Фикс: `isJobReady` (ready-семантика, зеркало Rust `has_ready_job`) — только running/pending
 * с наступившим run_at считается текущим прогоном.
 */
interface DigestState {
  latest: Digest | null;
  /** Идёт первичная загрузка из БД. */
  loading: boolean;
  /** Генерация поставлена в очередь и ещё не вернула новый дайджест. */
  generating: boolean;
  /** Текст ошибки (нет chat / сбой постановки) — для подсказки в UI. */
  error: string | null;
  /** `createdAt` дайджеста на момент клика «сгенерировать» — чтобы понять, что пришёл новый. */
  baseline: number | null;
  load: () => Promise<void>;
  generate: () => Promise<void>;
}

export const useDigestStore = create<DigestState>((set, get) => ({
  latest: null,
  loading: false,
  generating: false,
  error: null,
  baseline: null,

  async load() {
    set({ loading: true });
    try {
      const latest = await tauriApi.digest.latest();
      const { generating, baseline } = get();
      let stillGenerating = generating;
      if (generating) {
        const gotNew = latest != null && latest.createdAt !== baseline;
        // Завершилось: либо пришёл свежий дайджест, либо нет ГОТОВОЙ джобы (running/pending с
        // наступившим run_at) — гасим «Генерирую…», чтобы кнопка не висела вечно при сбое.
        // NB-4: НЕ jobActive — он считает и «завтрашнюю» recurring-pending «занятой» → вечный спиннер.
        const activeList = await tauriApi.scheduler.activeJobs();
        if (gotNew || !isJobReady('digest', activeList, Date.now())) stillGenerating = false;
      }
      set({ latest, loading: false, generating: stillGenerating });
    } catch {
      set({ loading: false });
    }
  },

  async generate() {
    set({ generating: true, error: null, baseline: get().latest?.createdAt ?? null });
    try {
      await tauriApi.digest.generate();
    } catch (e) {
      set({ generating: false, error: String(e) });
    }
  },
}));
