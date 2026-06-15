import { create } from 'zustand';

import type { LinkSuggestion } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';
import { activeBuffer, useWorkspaceStore } from './workspace';

/**
 * Предложения связей (Ф1-9, режим 1). Загружаются для активного файла; `accept` дописывает
 * `[[wikilink]]` в активный буфер, `dismiss` прячет (сессия). Отклонённые не возвращаются при
 * пересчёте (`load` повторно = «пересчитать»). Кэш/персист — позже (см. BACKLOG).
 */
interface SuggestState {
  path: string | null;
  items: LinkSuggestion[];
  loading: boolean;
  load: (path: string | null) => Promise<void>;
  dismiss: (target: string) => void;
  accept: (target: string) => void;
  /** Сбрасывает отклонённые цели (смена vault: относительные пути в новом vault — чужие). */
  clearDismissed: () => void;
}

// Отклонённые цели на путь (живёт в рамках сессии).
const dismissed = new Map<string, Set<string>>();

export const useSuggestStore = create<SuggestState>((set, get) => ({
  path: null,
  items: [],
  loading: false,

  async load(path) {
    set({ path, items: [], loading: Boolean(path) });
    if (!path) {
      set({ loading: false });
      return;
    }
    try {
      const all = await tauriApi.suggest.forFile(path);
      if (get().path !== path) return; // путь сменился, пока ждали
      const skip = dismissed.get(path) ?? new Set<string>();
      set({ items: all.filter((s) => !skip.has(s.path)), loading: false });
    } catch {
      if (get().path === path) set({ items: [], loading: false });
    }
  },

  dismiss(target) {
    const p = get().path;
    if (p) {
      const set_ = dismissed.get(p) ?? new Set<string>();
      set_.add(target);
      dismissed.set(p, set_);
    }
    set((s) => ({ items: s.items.filter((i) => i.path !== target) }));
  },

  accept(target) {
    const buf = activeBuffer(useWorkspaceStore.getState());
    if (buf) {
      const link = `[[${target.replace(/\.md$/, '')}]]`;
      const doc = buf.doc.endsWith('\n') ? `${buf.doc}${link}\n` : `${buf.doc}\n${link}\n`;
      useWorkspaceStore.getState().updateBufferDoc(buf.path, doc);
    }
    get().dismiss(target);
  },

  clearDismissed() {
    dismissed.clear();
  },
}));
