import { create } from 'zustand';

import { tauriApi, type MemoryFact } from '../lib/tauri-api';

/** Мягкий кап числа НЕ-пинов (D6, зеркалит backend `memory::MEM_CAP`): сверх него самые старые/редко
 *  используемые факты подсвечиваются для РУЧНОЙ чистки (не авто-эвикция). Пины не считаются. */
export const MEM_CAP = 200;

interface MemoryState {
  /** Все факты памяти агента (пины сверху, затем по дате — порядок с бэкенда). */
  facts: MemoryFact[];
  loading: boolean;
  /** Загрузить список из vault-БД. Ошибка → пустой список (без throw — панель не падает). */
  load: () => Promise<void>;
  /** Явно добавить факт (`source='explicit'`); пустой текст игнорируется. Перечитывает список. */
  add: (text: string) => Promise<void>;
  /** Пин/анпин; перечитывает (пин уезжает наверх). */
  setPinned: (id: number, pinned: boolean) => Promise<void>;
  /** Правка текста факта (ре-эмбеддинг на бэке); пустой текст — no-op. */
  edit: (id: number, text: string) => Promise<void>;
  /** Удалить факт (+ из индекса). */
  remove: (id: number) => Promise<void>;
}

/** Стор панели «Память ИИ» (MEM-4, AC-MEM-7): CRUD поверх tauri-команд `memory_*`. Мутаторы
 *  перечитывают список (память мала). `load()` защищён монотонным токеном — при перекрытии
 *  мутаций применяем только САМЫЙ свежий ответ (иначе раньше стартовавший `list()` мог бы
 *  резолвнуться последним и показать устаревший снимок). */
export const useMemoryStore = create<MemoryState>((set, get) => {
  let loadSeq = 0;
  return {
    facts: [],
    loading: false,
    async load() {
      const seq = ++loadSeq;
      set({ loading: true });
      try {
        const facts = await tauriApi.memory.list();
        if (seq === loadSeq) set({ facts });
      } catch {
        if (seq === loadSeq) set({ facts: [] });
      } finally {
        if (seq === loadSeq) set({ loading: false });
      }
    },
    async add(text) {
      const t = text.trim();
      if (!t) return;
      await tauriApi.memory.add(t, 'explicit');
      await get().load();
    },
    async setPinned(id, pinned) {
      await tauriApi.memory.setPinned(id, pinned);
      await get().load();
    },
    async edit(id, text) {
      const t = text.trim();
      if (!t) return;
      await tauriApi.memory.edit(id, t);
      await get().load();
    },
    async remove(id) {
      await tauriApi.memory.delete(id);
      await get().load();
    },
  };
});

/** D6-подсветка: id НЕ-пинов, которых стоит почистить вручную, когда не-пинов больше [`MEM_CAP`].
 *  Подсвечиваем «лишек» — наименее свежие (по `usedAt`, затем `createdAt`) сверх капа. Пустой Set,
 *  если переполнения нет (без ложных тревог на малой памяти). */
export function staleFactIds(facts: MemoryFact[]): Set<number> {
  const nonPins = facts.filter((f) => !f.pinned);
  if (nonPins.length <= MEM_CAP) return new Set();
  const lru = [...nonPins].sort((a, b) => a.usedAt - b.usedAt || a.createdAt - b.createdAt);
  return new Set(lru.slice(0, nonPins.length - MEM_CAP).map((f) => f.id));
}
