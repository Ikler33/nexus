import { create } from 'zustand';

import type { LinkSuggestion } from '../lib/tauri-api';
import { tauriApi } from '../lib/tauri-api';
import { activeBuffer, useWorkspaceStore } from './workspace';

/**
 * «Похожие заметки» (#35, режим дискавери): для активного файла — семантически близкие заметки,
 * ВКЛЮЧАЯ уже связанные (отличие от «Предложений связей»). Считается из готовых usearch-векторов
 * (без embedder-сервера, офлайн). Порог релевантности — настройка (D4): бэкенд отдаёт топ-N, вью
 * фильтрует по `threshold`. «Вставить связь» дописывает `[[wikilink]]`, но строку НЕ убирает (AC-RN-6).
 */
const THRESHOLD_KEY = 'nexus.related.threshold';

function loadThreshold(): number {
  try {
    const v = localStorage.getItem(THRESHOLD_KEY);
    if (v != null) {
      const n = Number(v);
      if (Number.isFinite(n) && n >= 0 && n <= 1) return n;
    }
  } catch {
    /* нет localStorage */
  }
  return 0; // дискавери: по умолчанию без отсечки (D4 — порог настраивается с v1)
}

interface RelatedState {
  path: string | null;
  /** Все полученные (топ-N); фильтр по порогу делает вью. */
  items: LinkSuggestion[];
  loading: boolean;
  /** Порог релевантности 0..1 (настройка). */
  threshold: number;
  load: (path: string | null) => Promise<void>;
  setThreshold: (t: number) => void;
  /** Вставить `[[target]]` в активный буфер (строку НЕ убираем — дискавери, AC-RN-6). */
  insertLink: (target: string) => void;
}

export const useRelatedStore = create<RelatedState>((set, get) => ({
  path: null,
  items: [],
  loading: false,
  threshold: loadThreshold(),

  async load(path) {
    set({ path, items: [], loading: Boolean(path) });
    if (!path) {
      set({ loading: false });
      return;
    }
    try {
      const all = await tauriApi.suggest.related(path);
      if (get().path !== path) return; // путь сменился, пока ждали
      set({ items: all, loading: false });
    } catch {
      if (get().path === path) set({ items: [], loading: false });
    }
  },

  setThreshold(t) {
    const v = Math.min(1, Math.max(0, t));
    try {
      localStorage.setItem(THRESHOLD_KEY, String(v));
    } catch {
      /* ignore */
    }
    set({ threshold: v });
  },

  insertLink(target) {
    const buf = activeBuffer(useWorkspaceStore.getState());
    if (!buf) return;
    const link = `[[${target.replace(/\.md$/, '')}]]`;
    const doc = buf.doc.endsWith('\n') ? `${buf.doc}${link}\n` : `${buf.doc}\n${link}\n`;
    useWorkspaceStore.getState().updateBufferDoc(buf.path, doc);
  },
}));

/** Видимые элементы (с учётом порога) — чистый помощник для вью/тестов. */
export function visibleRelated(items: LinkSuggestion[], threshold: number): LinkSuggestion[] {
  return items.filter((i) => i.score >= threshold);
}
