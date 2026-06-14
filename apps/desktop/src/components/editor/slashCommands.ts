import { type Completion, type CompletionSource, snippet } from '@codemirror/autocomplete';
import type { EditorView } from '@codemirror/view';
import i18n from '../../i18n/setup';
import { dateStamp } from '../../lib/daily';
import { TASK_MARKER } from '../../lib/editor/format';
import { useUIStore } from '../../stores/ui';

/**
 * Slash-команды (EDIT-6): ввод `/` в начале строки (или после пробела) открывает попап быстрых
 * вставок блоков. Реализован как CompletionSource — живёт во ВТОРОМ слоте единого `autocompletion()`
 * рядом с wikilink-источником (нельзя монтировать два autocompletion(), см. extensions.ts). Текстовые
 * вставки переиспользуют инварианты проекта: таск — TASK_MARKER (кликабелен в превью EDIT-5,
 * продолжается по Enter EDIT-3), дата — dateStamp (формат дневных заметок CAP-1). CSP-безопасно:
 * попап строит CM6 средствами DOM, без inline-стилей/скриптов.
 */

/** Триггер: `/` в начале строки или после пробела + буквы/цифры любого алфавита до курсора. */
const SLASH_RE = /(?:^|\s)\/([\p{L}\d]*)$/u;

type SlashApply = (view: EditorView, from: number, to: number) => void;

interface SlashItem {
  id: string;
  labelKey: string;
  detailKey?: string;
  apply: SlashApply;
}

/**
 * Блочный префикс: заменяет начало строки `[line.from, to]` на `prefix` + текст, что был до `/`.
 * «/h1» в начале строки → «# »; «текст /h1» → «# текст » (строка становится заголовком). Хвост
 * после курсора не трогается. Курсор — в конец вставки.
 */
function blockPrefix(prefix: string): SlashApply {
  return (view, from, to) => {
    const line = view.state.doc.lineAt(from);
    const before = view.state.sliceDoc(line.from, from); // текст строки до '/'
    const insert = prefix + before;
    view.dispatch({
      changes: { from: line.from, to, insert },
      selection: { anchor: line.from + insert.length },
    });
    view.focus();
  };
}

/** Инлайн-замена триггера `[from, to]` на текст (ленивый — дата вычисляется в момент вставки). */
function inlineText(make: () => string, cursorOffset?: number): SlashApply {
  return (view, from, to) => {
    const text = make();
    view.dispatch({
      changes: { from, to, insert: text },
      selection: { anchor: from + (cursorOffset ?? text.length) },
    });
    view.focus();
  };
}

/** Snippet-вставка (таблица): заменяет триггер шаблоном с Tab-навигацией по плейсхолдерам. */
function snippetApply(template: string): SlashApply {
  const run = snippet(template);
  return (view, from, to) => run(view, null, from, to);
}

/** Выбор шаблона: убрать триггер `/template` и показать модалку TemplatePicker (CAP-3). */
const openTemplatesApply: SlashApply = (view, from, to) => {
  view.dispatch({ changes: { from, to, insert: '' }, selection: { anchor: from } });
  view.focus();
  useUIStore.getState().openTemplates();
};

/** Декларативный реестр (порядок = порядок в попапе при пустом запросе). */
export const SLASH_ITEMS: SlashItem[] = [
  { id: 'h1', labelKey: 'slash.h1', apply: blockPrefix('# ') },
  { id: 'h2', labelKey: 'slash.h2', apply: blockPrefix('## ') },
  { id: 'h3', labelKey: 'slash.h3', apply: blockPrefix('### ') },
  { id: 'bullet', labelKey: 'slash.bullet', apply: blockPrefix('- ') },
  { id: 'numbered', labelKey: 'slash.numbered', apply: blockPrefix('1. ') },
  { id: 'task', labelKey: 'slash.task', apply: blockPrefix(TASK_MARKER) },
  {
    id: 'table',
    labelKey: 'slash.table',
    apply: snippetApply('| ${col1} | ${col2} |\n| --- | --- |\n| ${} | ${} |'),
  },
  { id: 'date', labelKey: 'slash.date', apply: inlineText(() => dateStamp(new Date())) },
  // TASK-2: дедлайн задачи — маркер 📅 + сегодня (распознаётся parseTaskMeta в дашборде задач).
  { id: 'due', labelKey: 'slash.due', apply: inlineText(() => `📅 ${dateStamp(new Date())} `) },
  // RECUR-1: повтор задачи — маркер 🔁 + интервал; при отметке порождается новая копия с продвинутым
  // дедлайном (parseRecurrence + toggleTaskAtLine). Курсор за словом «weekly» — легко заменить.
  { id: 'recur', labelKey: 'slash.recur', apply: inlineText(() => '🔁 weekly ') },
  // #4: формула KaTeX — `$$|$$` (двойной $ — одиночный отдан под валюту). Курсор между $$.
  { id: 'math', labelKey: 'slash.math', apply: inlineText(() => '$$$$', 2) },
  { id: 'link', labelKey: 'slash.link', apply: inlineText(() => '[]()', 1) },
  { id: 'callout', labelKey: 'slash.callout', apply: blockPrefix('> [!note] ') },
  { id: 'hr', labelKey: 'slash.hr', apply: blockPrefix('---') },
  { id: 'template', labelKey: 'slash.template', apply: openTemplatesApply },
];

/**
 * CompletionSource slash-команд. Срабатывает на `/`-триггере (но НЕ внутри незакрытого `[[wikilink`
 * — там работает wikilink-источник). `filter: false` — фильтруем реестр сами по введённому запросу
 * (по id и локализованному label), как wikilink-источник; так попап работает и для кириллицы.
 */
export function slashSource(): CompletionSource {
  return (ctx) => {
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = ctx.state.sliceDoc(line.from, ctx.pos);
    const m = SLASH_RE.exec(before);
    if (!m) return null;
    if (before.lastIndexOf('[[') > before.lastIndexOf(']]')) return null; // внутри [[wikilink
    const query = m[1].toLowerCase();
    const from = ctx.pos - m[1].length - 1; // позиция самого '/'
    const items = query
      ? SLASH_ITEMS.filter(
          (it) => it.id.includes(query) || i18n.t(it.labelKey).toLowerCase().includes(query),
        )
      : SLASH_ITEMS;
    if (!items.length) return null;
    const options: Completion[] = items.map((it) => ({
      label: i18n.t(it.labelKey),
      detail: it.detailKey ? i18n.t(it.detailKey) : undefined,
      type: 'keyword',
      apply: (view, _completion, f, t) => it.apply(view, f, t),
    }));
    return { from, options, filter: false };
  };
}
