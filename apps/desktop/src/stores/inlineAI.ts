import type { EditorView } from '@codemirror/view';
import { create } from 'zustand';

/**
 * InlineAI prompt-box (⌘/ или `/ai`, дизайн Qasr `editor.jsx`): свободный запрос к LLM с заземлением на
 * текущую заметку → стрим → вставка в редактор. Открыт максимум в ОДНОЙ группе (по `openGroupId` —
 * бар рендерит только её `GroupPane`). Свободный промпт ОРТОГОНАЛЕН ghost-тексту (`inlineGhost.ts`,
 * continue/rewrite/summarize): другой триггер, другой UX, разные сторы.
 */
interface InlineAIState {
  /** Группа, в которой открыт prompt-box (`null` — закрыт). */
  openGroupId: string | null;
  /**
   * CM6-редактор, ОТКУДА открыли бар (захвачен на триггере ⌘//`/ai`). Вставка идёт именно в него, а не
   * в «глобально активный» view: при сплитах фокус мог уйти в другой пейн (ревью: иначе текст попадал
   * бы в чужой редактор). `null` — нет живого view (фолбэк — дописать в конец буфера).
   */
  view: EditorView | null;
  /** Открыть бар в группе `groupId`, целясь вставкой в `view` (перекрывает прежний — один активный). */
  open: (groupId: string, view: EditorView | null) => void;
  /** Закрыть бар. Идемпотентно. */
  close: () => void;
}

export const useInlineAIStore = create<InlineAIState>((set) => ({
  openGroupId: null,
  view: null,
  open: (groupId, view) => set({ openGroupId: groupId, view }),
  close: () => set({ openGroupId: null, view: null }),
}));
