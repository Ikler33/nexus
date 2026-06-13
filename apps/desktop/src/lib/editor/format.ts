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

/** Один шаг тоггла таска для строки: `- [ ]`↔`- [x]`; строка без чекбокса (буллет или обычный
 *  текст, в т.ч. пустая) → `- [ ] …`. Отступ сохраняется. */
function transformTaskLine(line: string): string {
  const m = /^(\s*)(?:[-*+] )?(?:\[\s*([ xX])\s*\] ?)?(.*)$/.exec(line);
  if (!m) return line;
  const [, indent, check, rest] = m;
  if (check != null) {
    const checked = check === 'x' || check === 'X';
    return `${indent}- [${checked ? ' ' : 'x'}] ${rest}`;
  }
  return `${indent}- [ ] ${rest}`;
}

/**
 * Тоггл markdown-таска на строке(ах) выделения (EDIT-2, ⌘L): `- [ ]`↔`- [x]`; строка без чекбокса
 * (буллет/обычный текст/пустая) превращается в таск `- [ ] …`. Каждая выделенная строка
 * обрабатывается независимо. Правка обычным dispatch (без externalSync) → редактор пометит dirty.
 */
export function toggleTask(view: EditorView): boolean {
  const { state } = view;
  const { from, to } = state.selection.main;
  const firstLine = state.doc.lineAt(from);
  let lastLine = state.doc.lineAt(to);
  // Выделение, кончающееся в начале строки ниже (col 0), не должно «цеплять» эту строку:
  // классическая ловушка построчных тогглеров — выделили 2 строки, таск появляется на 3-й.
  if (to > from && to === lastLine.from && lastLine.number > firstLine.number) {
    lastLine = state.doc.line(lastLine.number - 1);
  }
  const out: string[] = [];
  for (let n = firstLine.number; n <= lastLine.number; n++) {
    out.push(transformTaskLine(state.doc.line(n).text));
  }
  view.dispatch({
    changes: { from: firstLine.from, to: lastLine.to, insert: out.join('\n') },
  });
  view.focus();
  return true;
}
