import { type EditorState } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';

/**
 * Обёрнуто ли выделение [from,to) ровно маркером `marker` снаружи. Анти-коллизия `*` vs `**`:
 * маркер не считается обрамлением, если он часть более длинного прогона того же символа (выделение
 * «bold» внутри `**bold**` НЕ обёрнуто курсивом `*` — иначе тоггл курсива молча снёс бы жирный).
 */
function isWrappedBy(state: EditorState, from: number, to: number, marker: string): boolean {
  const len = marker.length;
  if (from - len < 0 || to + len > state.doc.length) return false;
  if (state.sliceDoc(from - len, from) !== marker || state.sliceDoc(to, to + len) !== marker) {
    return false;
  }
  const ch = marker[0];
  const moreBefore = from - len - 1 >= 0 && state.sliceDoc(from - len - 1, from - len) === ch;
  const moreAfter = to + len < state.doc.length && state.sliceDoc(to + len, to + len + 1) === ch;
  return !moreBefore && !moreAfter;
}

/**
 * Тоггл markdown-обрамления ОСНОВНОГО выделения симметричным маркером (EDIT-1):
 * `**` — жирный (⌘B), `*` — курсив (⌘⇧I; ⌘I занят inline-LLM, IL-2). Если выделение уже обёрнуто
 * этим маркером — снимаем обрамление; иначе оборачиваем (в т.ч. добавляем курсив поверх жирного →
 * `***…***`). Пустое выделение → вставляем пару маркеров и ставим курсор между («**|**» — пиши сразу).
 * Правка идёт обычным dispatch (без externalSync) → редактор пометит dirty + автосейв. Мультикурсор —
 * отдельная доработка (обрабатывается только `selection.main`).
 */
export function toggleWrap(view: EditorView, marker: string): boolean {
  const { state } = view;
  const { from, to } = state.selection.main;
  const len = marker.length;
  if (isWrappedBy(state, from, to, marker)) {
    // Снять обрамление вокруг выделения (контент сдвигается влево на длину маркера).
    view.dispatch({
      changes: [
        { from: from - len, to: from },
        { from: to, to: to + len },
      ],
      selection: { anchor: from - len, head: to - len },
    });
  } else {
    // Обернуть; пустое выделение → курсор окажется между вставленными маркерами.
    view.dispatch({
      changes: [
        { from, insert: marker },
        { from: to, insert: marker },
      ],
      selection: { anchor: from + len, head: to + len },
    });
  }
  view.focus();
  return true;
}
