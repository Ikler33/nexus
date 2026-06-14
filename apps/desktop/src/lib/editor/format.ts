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

export function toggleTaskAtLine(doc: string, line: number, today?: string): string | null {
  const lines = doc.split('\n');
  if (line < 1 || line > lines.length) return null;
  const cur = lines[line - 1];
  const m = TASK_LINE_RE.exec(cur);
  if (!m) return null;
  const checking = m[2] === ' '; // unchecked → checked
  const toggled = m[1] + (checking ? 'x' : ' ') + m[3];
  // RECUR-1: завершение повторяющейся задачи (🔁) — текущая помечается done (исторический след с её
  // дедлайном), а НОВАЯ открытая копия с продвинутым дедлайном вставляется ВЫШЕ. Только при отметке
  // (unchecked→checked); снятие галки или не-рекуррентный таск — простой флип (как было). `today` —
  // база для повторов без дедлайна (инъекция для тестов; по умолчанию — сегодня).
  if (checking) {
    const recur = parseRecurrence(cur);
    if (recur) {
      const base = parseTaskMeta(cur).due ?? today ?? fmtDate(new Date());
      const next = withDue(cur, addInterval(base, recur)); // копия cur ([ ]) с новым дедлайном
      lines[line - 1] = toggled;
      lines.splice(line - 1, 0, next);
      return lines.join('\n');
    }
  }
  lines[line - 1] = toggled;
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

/** Мета задачи (TASK-2): дедлайн и приоритет, распознанные из ТЕКСТА задачи. Парсятся ОТДЕЛЬНО от
 *  TASK_LINE_RE (не зеркалятся в Rust) — поэтому диск и буфер обрабатываются единообразно в панели. */
export interface TaskMeta {
  /** Нормализованный дедлайн 'YYYY-MM-DD' (или отсутствует). */
  due?: string;
  /** Приоритет 1 (высший) … 3 (низший). */
  priority?: 1 | 2 | 3;
}

/** Дедлайн в трёх формах: `📅 2026-06-20`, `@due(2026-06-20)`, `due:2026-06-20`. `\b` перед `due:`,
 *  чтобы `overdue:2026-06-20` не ловилось как дедлайн. */
const DUE_RE = /(?:📅\s*|@due\(|\bdue:)(\d{4})-(\d{1,2})-(\d{1,2})\)?/;
/** Приоритет текстом: `!p1`..`!p3` (граница `\b`, чтобы `!important`/`top1` не ловились). */
const PRIO_RE = /!p([123])\b/i;
const PRIO_EMOJI: Record<string, 1 | 2 | 3> = { '⏫': 1, '🔼': 2, '🔽': 3 };

/** 'YYYY-MM-DD' из Date по локальному времени (zero-pad — нужен для лексикографики бакетов). */
function fmtDate(dt: Date): string {
  return `${dt.getFullYear()}-${String(dt.getMonth() + 1).padStart(2, '0')}-${String(dt.getDate()).padStart(2, '0')}`;
}

/** Валидирует Y-M-D через round-trip Date (отсекает 2026-13-40 и 2026-02-30) → нормализ. строка/undefined. */
function normalizeDate(y: number, m: number, d: number): string | undefined {
  if (m < 1 || m > 12 || d < 1 || d > 31) return undefined;
  const dt = new Date(y, m - 1, d);
  if (dt.getFullYear() !== y || dt.getMonth() !== m - 1 || dt.getDate() !== d) return undefined;
  return fmtDate(dt);
}

/** Извлекает дедлайн+приоритет из текста задачи (TASK-2). Эмодзи-приоритет приоритетнее `!pN`. */
export function parseTaskMeta(text: string): TaskMeta {
  const meta: TaskMeta = {};
  const dm = DUE_RE.exec(text);
  if (dm) meta.due = normalizeDate(Number(dm[1]), Number(dm[2]), Number(dm[3]));
  for (const [emoji, p] of Object.entries(PRIO_EMOJI)) {
    if (text.includes(emoji)) {
      meta.priority = p;
      break;
    }
  }
  if (meta.priority == null) {
    const pm = PRIO_RE.exec(text);
    if (pm) meta.priority = Number(pm[1]) as 1 | 2 | 3;
  }
  return meta;
}

/** Прибавляет `n` дней к ISO-дате (через Date — корректно для месяцев/лет/високосных). */
function addDays(stamp: string, n: number): string {
  const [y, m, d] = stamp.split('-').map(Number);
  return fmtDate(new Date(y, m - 1, d + n));
}

/** Единица повторения задачи (RECUR-1). */
type RecurUnit = 'day' | 'week' | 'month' | 'year';
/** Распознанный повтор: кратность `n` (≥1) × единица. */
export interface Recurrence {
  n: number;
  unit: RecurUnit;
}
/** Повтор `🔁` в тексте задачи: `daily`/`weekly`/`monthly`/`yearly` или `every N day|week|month|year(s)`
 *  (англ. ключевые слова — как в Obsidian Tasks; регистронезависимо, пробел после 🔁 необязателен). */
const RECUR_RE = /🔁\s*(?:every\s+(\d+)\s+(day|week|month|year)s?|(daily|weekly|monthly|yearly))/i;
const RECUR_WORD: Record<string, RecurUnit> = {
  daily: 'day',
  weekly: 'week',
  monthly: 'month',
  yearly: 'year',
};

/** Верхняя граница кратности повтора: отсекает абсурд (`every 99999999 months`), который переполнил бы
 *  Date и дал бы `NaN-NaN-NaN` в файле. 10000 покрывает любой реальный интервал, оставаясь в диапазоне Date. */
const MAX_RECUR_N = 10_000;

/** Извлекает повтор из текста задачи (RECUR-1) или `null`, если 🔁-маркера нет. */
export function parseRecurrence(text: string): Recurrence | null {
  const m = RECUR_RE.exec(text);
  if (!m) return null;
  if (m[1]) {
    const n = Math.min(MAX_RECUR_N, Math.max(1, Number(m[1])));
    return { n, unit: m[2].toLowerCase() as RecurUnit };
  }
  return { n: 1, unit: RECUR_WORD[m[3].toLowerCase()] };
}

/** Устанавливает абсолютный индекс месяца `targetMonth` (0-based от эпохи Date, допускает >11/<0 —
 *  переезд в годы), КЛЕМПЯ день до последнего дня целевого месяца: Jan 31 +1мес → Feb 28 (не Mar 3),
 *  Feb 29 +1год → Feb 28. Как в Obsidian Tasks — серия не «перескакивает» переполнившийся месяц. */
function setMonthClamped(dt: Date, targetMonth: number): void {
  const day = dt.getDate();
  dt.setDate(1); // иначе setMonth переполнил бы день и «переехал» вперёд
  dt.setMonth(targetMonth);
  const lastDay = new Date(dt.getFullYear(), dt.getMonth() + 1, 0).getDate();
  dt.setDate(Math.min(day, lastDay));
}

/** Продвигает ISO-дату `stamp` на интервал `recur` (через Date — корректно для месяцев/лет/високосных).
 *  При невалидном результате (теоретическое переполнение) возвращает `stamp` без сдвига — лучше не
 *  продвинуть, чем записать в файл битую дату. */
function addInterval(stamp: string, recur: Recurrence): string {
  const [y, m, d] = stamp.split('-').map(Number);
  const dt = new Date(y, m - 1, d);
  switch (recur.unit) {
    case 'day':
      dt.setDate(dt.getDate() + recur.n);
      break;
    case 'week':
      dt.setDate(dt.getDate() + 7 * recur.n);
      break;
    case 'month':
      setMonthClamped(dt, dt.getMonth() + recur.n);
      break;
    case 'year':
      setMonthClamped(dt, dt.getMonth() + 12 * recur.n);
      break;
  }
  return Number.isNaN(dt.getTime()) ? stamp : fmtDate(dt);
}

/** Возвращает строку задачи с дедлайном `newDue`: заменяет дату в существующем токене дедлайна
 *  (📅/@due(/due:) — сохраняя форму) либо дописывает `📅 newDue` (перед хвостовым `\r`, если есть). */
function withDue(line: string, newDue: string): string {
  if (DUE_RE.test(line)) {
    return line.replace(DUE_RE, (full) => full.replace(/\d{4}-\d{1,2}-\d{1,2}/, newDue));
  }
  return line.replace(/(\r?)$/, ` 📅 ${newDue}$1`);
}

/** Временной бакет задачи по дедлайну относительно `today` (обе — нормализ. 'YYYY-MM-DD', сравнение
 *  лексикографическое = хронологическое). Граница недели включительно (+7 дней). Без даты → 'none'. */
export function bucketOf(
  due: string | undefined,
  today: string,
): 'overdue' | 'today' | 'week' | 'later' | 'none' {
  if (!due) return 'none';
  if (due < today) return 'overdue';
  if (due === today) return 'today';
  return due <= addDays(today, 7) ? 'week' : 'later';
}

/** Похоже ли выделение на URL/почту — тогда оно идёт в адрес ссылки, а не в её текст.
 *  `\S+` (не `\S*`) — после схемы обязателен контент: голый `www.`/`tel:`/`https://` — это текст. */
const LINK_TARGET_RE = /^(https?:\/\/|mailto:|tel:|www\.)\S+$/i;

/** Экранирует `[`/`]` в тексте ссылки, чтобы выделение со скобками не рвало `[…]`. */
const escapeLinkText = (s: string): string => s.replace(/[[\]]/g, '\\$&');

/** Экранирует `(`/`)` в адресе: в `(…)`-назначении CommonMark скобки должны быть сбалансированы или
 *  экранированы — иначе URL вида `…/Foo_(bar)` рвёт `[](…)`. */
const escapeLinkUrl = (s: string): string => s.replace(/[()]/g, '\\$&');

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
    insert = `[](${escapeLinkUrl(sel)})`;
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
