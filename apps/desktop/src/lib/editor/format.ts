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

/** Канонический маркер незавершённого таска. Общий для тоггла (EDIT-2), вставки slash-командой
 *  (EDIT-6) и т.п. — чтобы вставленный таск гарантированно подпадал под TASK_LINE_RE (кликабелен
 *  в превью EDIT-5, продолжается по Enter EDIT-3). */
export const TASK_MARKER = '- [ ] ';

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
  return `${indent}${TASK_MARKER}${rest}`;
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

/**
 * Чистый тоггл состояния таска на 1-based строке `line` документа `doc` (EDIT-5: клик по чекбоксу
 * в превью): `[ ]`↔`[x]`/`[X]`. Возвращает новый текст или `null`, если строка вне диапазона или
 * не таск-пункт — защита от дрейфа номера строки между рендером превью и кликом (доку могли изменить).
 */
// group3 = `\][^\n]*` (а не `\].*`): `.` не матчит `\r`, поэтому `$` без флага `m` не достаёт конца
// CRLF-строки → задачи в файлах с Windows-переводами строк выпадали из дашборда и не тогглились
// из буфера (Rust-скан через `.lines()` срезает `\r` и расходился с фронтом). `[^\n]*` вбирает
// хвостовой `\r` в group3 → тоггл сохраняет перевод строки, а parseTasks обрезает его через `.trim()`.
const TASK_LINE_RE = /^(\s*(?:[-*+]|\d+[.)])\s+\[)([ xX])(\][^\n]*)$/;

export function toggleTaskAtLine(doc: string, line: number): string | null {
  const lines = doc.split('\n');
  if (line < 1 || line > lines.length) return null;
  const m = TASK_LINE_RE.exec(lines[line - 1]);
  if (!m) return null;
  lines[line - 1] = m[1] + (m[2] === ' ' ? 'x' : ' ') + m[3];
  return lines.join('\n');
}

/**
 * Тогглится ли строка через {@link toggleTaskAtLine} (EDIT-5). Превью по этому решает: рисовать
 * интерактивный чекбокс или отдать read-only (напр. таск в цитате `> - [ ]` — GFM-таск, но исходная
 * строка с префиксом `>` не подпадает под TASK_LINE_RE → честный disabled, а не мёртвый «кликабельный»).
 */
export function isTaskLine(text: string): boolean {
  return TASK_LINE_RE.test(text);
}

/** Одна задача из текста заметки (TASK-1): 1-based номер строки, текст после `]`, состояние. */
export interface Task {
  line: number;
  text: string;
  checked: boolean;
}

/**
 * Извлекает все markdown-задачи из текста заметки (TASK-1, дашборд). Переиспользует тот же
 * TASK_LINE_RE, что toggleTaskAtLine/isTaskLine, — фронтовое зеркало бэкенд-парсера parse_task_line
 * (src-tauri/src/commands/tasks.rs). Нужна для наложения грязных буферов поверх дискового списка.
 */
export function parseTasks(doc: string): Task[] {
  const out: Task[] = [];
  const lines = doc.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const m = TASK_LINE_RE.exec(lines[i]);
    if (m) out.push({ line: i + 1, checked: m[2] !== ' ', text: m[3].replace(/^\]\s?/, '').trim() });
  }
  return out;
}

/** Похоже ли выделение на URL/почту — тогда оно идёт в адрес ссылки, а не в её текст.
 *  `\S+` (не `\S*`) — после схемы обязателен контент: голый `www.`/`tel:`/`https://` — это текст. */
const LINK_TARGET_RE = /^(https?:\/\/|mailto:|tel:|www\.)\S+$/i;

/** Экранирует `[`/`]` в тексте ссылки, чтобы выделение со скобками не рвало `[…]`. */
const escapeLinkText = (s: string): string => s.replace(/[[\]]/g, '\\$&');

/**
 * Вставка markdown-ссылки на основном выделении (EDIT-4, ⌘K). Три случая, курсор ставится туда,
 * куда логично печатать дальше:
 *  - пустое выделение → `[]()`, курсор в тексте `[|]`;
 *  - выделение похоже на URL → `[](url)`, курсор в тексте `[|]` (адрес уже есть);
 *  - выделен текст → `[текст]()`, курсор в адресе `(|)` (готов вставить/печатать ссылку).
 * Правка обычным dispatch (без externalSync) → редактор пометит dirty + автосейв.
 */
export function insertLink(view: EditorView): boolean {
  const { state } = view;
  const { from, to } = state.selection.main;
  const sel = state.sliceDoc(from, to);
  let insert: string;
  let caret: number; // абсолютная позиция курсора после вставки
  if (sel === '') {
    insert = '[]()';
    caret = from + 1; // между скобок текста: [|]
  } else if (LINK_TARGET_RE.test(sel.trim())) {
    insert = `[](${sel})`;
    caret = from + 1; // текст пуст, адрес = выделение: [|](url)
  } else {
    const text = escapeLinkText(sel);
    insert = `[${text}]()`;
    caret = from + text.length + 3; // сразу после `](`: [текст](|)
  }
  view.dispatch({
    changes: { from, to, insert },
    selection: { anchor: caret },
  });
  view.focus();
  return true;
}
