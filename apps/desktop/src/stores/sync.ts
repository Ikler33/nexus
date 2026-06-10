import { create } from 'zustand';

/**
 * Лёгкое глобальное состояние git-синка (DP-14): конфликт-пилюля статусбара должна жить и после
 * закрытия SyncPanel, пока merge не разрешён. Источники: SyncPanel (sync → `merge-required`),
 * ConflictResolver (preview даёт число файлов; успешный apply снимает флаг).
 */
interface SyncState {
  /** Pull требует merge — показываем конфликт-пилюлю в статусбаре. */
  mergeRequired: boolean;
  /** Число конфликтных файлов из merge-preview (`null` — ещё не считали). */
  conflictFiles: number | null;
  setMergeRequired: (v: boolean) => void;
  setConflictFiles: (n: number | null) => void;
}

export const useSyncStore = create<SyncState>((set) => ({
  mergeRequired: false,
  conflictFiles: null,
  setMergeRequired: (mergeRequired) =>
    set((s) => ({ mergeRequired, conflictFiles: mergeRequired ? s.conflictFiles : null })),
  setConflictFiles: (conflictFiles) => set({ conflictFiles }),
}));
