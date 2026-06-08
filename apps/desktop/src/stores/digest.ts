import { create } from 'zustand';

import type { Digest } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';

/**
 * «Дайджест изменений» (#35, ADR-007 slice 4): последний LLM-дайджест недавно изменённых заметок.
 * Генерация асинхронна (джоба планировщика): `generate()` ставит её в очередь, а готовый результат
 * прилетает через `load()` по событию `jobs:changed` (см. App). `generating` снимается, когда из БД
 * приходит дайджест свежее того, что был на момент клика (baseline).
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
        // Завершилось: либо пришёл свежий дайджест, либо джоба больше не активна (упала/таймаут/no-op)
        // — гасим «Генерирю…», чтобы кнопка не висела вечно при сбое (а не только при успехе).
        if (gotNew || !(await tauriApi.scheduler.jobActive('digest'))) stillGenerating = false;
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
