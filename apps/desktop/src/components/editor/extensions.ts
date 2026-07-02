import {
  autocompletion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from '@codemirror/autocomplete';
import { defaultHighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { markdown } from '@codemirror/lang-markdown';
import { type Extension, RangeSetBuilder } from '@codemirror/state';
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from '@codemirror/view';
import type { NoteRef } from '../../lib/tauri-api';
import { noteName } from '../../stores/vault';
import { slashSource } from './slashCommands';
import { tagCompletionQuery } from './tag-complete';

// `[[Target]]`, `[[Target#H|Alias]]`, `![[Embed]]`
const WIKILINK_RE = /(!?)\[\[([^\]\n]+?)\]\]/g;
// `#tag` (минимум одна буква; перед — начало или пробел)
const TAG_RE = /(^|\s)(#[\p{L}\d/_-]*\p{L}[\p{L}\d/_-]*)/gu;

const wikilinkMark = Decoration.mark({ class: 'cm-wikilink' });
const tagMark = Decoration.mark({ class: 'cm-tag' });

/** Нормализует цель wiki-ссылки: срезает `|alias` и `#heading`. */
export function normalizeTarget(inner: string): string {
  return inner.split('|')[0].split('#')[0].trim();
}

/**
 * Диапазон видимого ЛЕЙБЛА внутри `inner` (без обрамляющих `[[`/`]]`): alias после `|`, иначе target
 * без `#heading`. Возвращает смещения [start, end) ОТНОСИТЕЛЬНО `inner` (НЕ тримит — границы по сырому
 * тексту, чтобы скрытые префикс/суффикс точно стыковались с лейблом, без «съеденных» пробелов).
 * Зеркалит `remarkNexus.wikilinkLabel` (preview-режим) по смыслу. Если лейбл пуст (напр. `[[|x]]`
 * без target ИЛИ `[[#H]]`) — фолбэк на весь `inner` (ничего не прячем внутри, только скобки).
 */
export function wikilinkLabelRange(inner: string): { start: number; end: number } {
  const bar = inner.indexOf('|');
  if (bar >= 0) {
    // Алиас: видимое — всё после `|` (как есть, до конца inner).
    const start = bar + 1;
    if (start < inner.length) return { start, end: inner.length };
    return { start: 0, end: inner.length }; // пустой алиас → не прячем внутренности
  }
  // Без алиаса: видимое — target до `#heading`.
  const hash = inner.indexOf('#');
  if (hash > 0) return { start: 0, end: hash };
  if (hash === 0) return { start: 0, end: inner.length }; // только `#H` без target → не прячем
  return { start: 0, end: inner.length };
}

/** Скрываемый диапазон синтаксиса live-preview (для `Decoration.replace`). */
export interface LpHiddenRange {
  from: number;
  to: number;
}

/**
 * Live-preview вики-ссылок (ЧИСТАЯ, тестируемая): по тексту и выделению возвращает диапазоны
 * СИНТАКСИСА для скрытия (`[[`, префикс `Target|`/суффикс `#heading`, `]]`), оставляя видимым лейбл.
 *
 * - Эмбеды `![[...]]` ПРОПУСКАЮТСЯ (не скрываем — `m[1] === '!'`).
 * - РАСКРЫТИЕ под курсором: если [selFrom, selTo] пересекает диапазон ссылки СТРОГО ВНУТРИ
 *   (края исключены, EDFIX-4) — ничего не прячем (видно сырой `[[inner]]`, редактируемо; при наборе
 *   нового `[[…` курсор внутри → не печатаешь вслепую). Курсор ровно на краю (== matchStart или
 *   == matchEnd) НЕ раскрывает: закрыл `]]` → ссылка схлопнулась сразу.
 * - `offset` — абсолютное смещение `text` в документе (для построения по `visibleRanges` кусками).
 */
export function buildLivePreviewRanges(
  text: string,
  selFrom: number,
  selTo: number,
  offset = 0,
): LpHiddenRange[] {
  const out: LpHiddenRange[] = [];
  for (const m of text.matchAll(WIKILINK_RE)) {
    if (m[1] === '!') continue; // эмбед — оставляем как есть
    const inner = m[2];
    const matchStart = offset + m.index!;
    const matchEnd = matchStart + m[0].length;
    // Раскрытие под курсором: выделение пересекает (matchStart, matchEnd) СТРОГО (края ИСКЛЮЧЕНЫ,
    // EDFIX-4 КОРЕНЬ 4). Закрыл `]]` (курсор == matchEnd) → ссылка схлопывается в лейбл/алиас СРАЗУ —
    // мгновенная обратная связь «алиас работает» (прежние inclusive-края держали только что набранную
    // ссылку сырой, и владелец считал алиасы сломанными). Набор ВНУТРИ `[[…]]` (автозакрытие скобок
    // ставит курсор строго внутри) по-прежнему держит ссылку раскрытой → не печатаешь вслепую;
    // клик/стрелки внутрь работают через atomic-переходы (EditorView.atomicRanges ниже) — курсор,
    // перепрыгнув скрытый край, оказывается строго внутри и раскрывает синтаксис.
    if (selFrom < matchEnd && selTo > matchStart) continue;
    const innerStart = matchStart + 2; // после `[[`
    const { start, end } = wikilinkLabelRange(inner);
    const labelFrom = innerStart + start;
    const labelTo = innerStart + end;
    // Скрываем: открывающие `[[` + префикс до лейбла; суффикс после лейбла + закрывающие `]]`.
    if (labelFrom > matchStart) out.push({ from: matchStart, to: labelFrom }); // `[[` (+`Target|`)
    if (matchEnd > labelTo) out.push({ from: labelTo, to: matchEnd }); // (`#heading`+) `]]`
  }
  return out;
}

interface DecoRange {
  from: number;
  to: number;
  deco: Decoration;
}

function collectDecorations(view: EditorView): DecorationSet {
  const ranges: DecoRange[] = [];
  for (const { from, to } of view.visibleRanges) {
    const text = view.state.sliceDoc(from, to);
    for (const m of text.matchAll(WIKILINK_RE)) {
      ranges.push({
        from: from + m.index!,
        to: from + m.index! + m[0].length,
        deco: wikilinkMark,
      });
    }
    for (const m of text.matchAll(TAG_RE)) {
      const start = m.index! + m[1].length;
      ranges.push({ from: from + start, to: from + start + m[2].length, deco: tagMark });
    }
  }
  ranges.sort((a, b) => a.from - b.from || a.to - b.to);
  const builder = new RangeSetBuilder<Decoration>();
  let last = -1;
  for (const r of ranges) {
    if (r.from >= last) {
      builder.add(r.from, r.to, r.deco);
      last = r.to;
    }
  }
  return builder.finish();
}

/** Подсветка `[[wikilink]]` и `#tag` (декорации поверх видимой области). */
const decorationPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = collectDecorations(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged) {
        this.decorations = collectDecorations(u.view);
      }
    }
  },
  { decorations: (v) => v.decorations },
);

/** Скрытый replace-декоратор синтаксиса вики-ссылки (атомарный — курсор перепрыгивает скрытое). */
const lpHide = Decoration.replace({});

/** Строит набор скрывающих декораций live-preview по видимой области + текущему выделению. */
function collectLivePreview(view: EditorView): DecorationSet {
  const { from: selFrom, to: selTo } = view.state.selection.main;
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of view.visibleRanges) {
    const text = view.state.sliceDoc(from, to);
    for (const r of buildLivePreviewRanges(text, selFrom, selTo, from)) {
      builder.add(r.from, r.to, lpHide);
    }
  }
  return builder.finish();
}

/**
 * Live-preview вики-ссылок (СУТЬ, Obsidian-style): скрывает `[[ ]]`-скобки/`Target|`-префикс/`#heading`-
 * суффикс через `Decoration.replace`, оставляя видимым только лейбл; раскрывает полный синтаксис под
 * курсором (обновляется на `docChanged || viewportChanged || selectionSet`). Это ДИСПЛЕЙ-декорация —
 * исходный текст `[[Файл]]` НЕ меняется. `provide` отдаёт скрытые диапазоны как АТОМАРНЫЕ → курсор
 * корректно перепрыгивает скрытое (не «застревает» внутри). Включается под Compartment (Editor.tsx).
 */
export const wikilinkLivePreview: Extension = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = collectLivePreview(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged || u.selectionSet) {
        this.decorations = collectLivePreview(u.view);
      }
    }
  },
  {
    decorations: (v) => v.decorations,
    // Атомарность: курсор/выделение перепрыгивает скрытый синтаксис как единый блок (UX live-preview).
    provide: (plugin) =>
      EditorView.atomicRanges.of((view) => view.plugin(plugin)?.decorations ?? Decoration.none),
  },
);

/** Клик по `[[wikilink]]` → навигация (через `onOpenLink`). */
function wikilinkClick(onOpenLink: () => ((target: string) => void) | undefined): Extension {
  return EditorView.domEventHandlers({
    mousedown(event, view) {
      if (event.button !== 0) return false;
      const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
      if (pos == null) return false;
      const line = view.state.doc.lineAt(pos);
      const rel = pos - line.from;
      for (const m of line.text.matchAll(WIKILINK_RE)) {
        const start = m.index!;
        const end = start + m[0].length;
        if (rel >= start && rel <= end) {
          const target = normalizeTarget(m[2]);
          const handler = onOpenLink();
          if (target && handler) {
            event.preventDefault();
            handler(target);
            return true;
          }
        }
      }
      return false;
    },
  });
}

/** Автокомплит имён заметок внутри `[[…` — асинхронный запрос топ-N к бэкенду (кросс-план #22):
 * вместо полного списка vault в памяти каждый ввод спрашивает отфильтрованный срез (бэкенд ранжирует
 * префикс-совпадения выше). `filter: false` — CM6 не пере-фильтрует уже отобранное. Чистый
 * CompletionSource — монтируется в ЕДИНЫЙ autocompletion() рядом со slash-источником (EDIT-6). */
function wikilinkSource(fetchNotes: (q: string) => Promise<NoteRef[]>): CompletionSource {
  return async (ctx: CompletionContext): Promise<CompletionResult | null> => {
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = ctx.state.sliceDoc(line.from, ctx.pos);
    const m = /\[\[([^\]\n]*)$/.exec(before);
    if (!m) return null;
    const from = ctx.pos - m[1].length;
    const notes = await fetchNotes(m[1]);
    const options = notes.map((n) => ({
      label: noteName(n.path),
      detail: n.title ?? n.path,
      type: 'class',
    }));
    return { from, options, filter: false };
  };
}

const editorTheme = EditorView.theme({
  '&': { height: '100%', fontSize: 'var(--text-base)' },
  '.cm-scroller': { fontFamily: 'var(--font-editor)', lineHeight: 'var(--leading-normal)' },
  // Читаемая ширина строки (настройка «Редактор»): колонка ограничивается и центрируется через
  // CSS-переменную `--editor-max-width` (stores/prefs.ts). `none` → полная ширина (как было).
  '.cm-content': {
    padding: 'var(--space-4) 0',
    maxWidth: 'var(--editor-max-width, none)',
    marginInline: 'auto',
  },
  '.cm-wikilink': { color: 'var(--color-link)', cursor: 'pointer' },
  '.cm-tag': { color: 'var(--color-tag)' },
  // Inline-LLM ghost-text (IL-2): приглушённый курсивный текст предложения у курсора.
  '.cm-inline-ghost': {
    color: 'var(--color-text-faint)',
    fontStyle: 'italic',
    opacity: '0.75',
    whiteSpace: 'pre-wrap',
  },
  // Индикатор «генерируется» (до первого токена, AC-IL-1): акцентный пульсирующий чип.
  '.cm-inline-ghost-pending': {
    color: 'var(--color-accent)',
    opacity: '0.85',
    animation: 'nexus-ghost-pulse 1.1s ease-in-out infinite',
  },
  '@keyframes nexus-ghost-pulse': {
    '0%, 100%': { opacity: '0.4' },
    '50%': { opacity: '0.9' },
  },
  // Подсказка accept/reject у завершённого предложения (AC-IL-10).
  '.cm-inline-ghost-hint': {
    fontSize: 'var(--text-xs)',
    fontStyle: 'normal',
    opacity: '0.6',
    whiteSpace: 'nowrap',
  },
  // Inline-ошибка у курсора (AC-IL-7): без модала, ненавязчиво.
  '.cm-inline-ghost-error': {
    color: 'var(--color-danger, oklch(0.6 0.2 25))',
    fontStyle: 'normal',
    opacity: '0.95',
  },
  // Тулбар по выделению (IL-3, D4): чип с кнопками над выделением.
  '.cm-inline-toolbar': {
    display: 'flex',
    gap: '2px',
    padding: '3px',
    background: 'var(--color-bg-elevated)',
    border: '1px solid var(--color-border-strong)',
    borderRadius: 'var(--radius-md)',
    boxShadow: 'var(--elevation-2)',
  },
  '.cm-inline-toolbar-btn': {
    font: 'inherit',
    fontSize: 'var(--text-xs)',
    color: 'var(--color-text)',
    background: 'transparent',
    border: 'none',
    borderRadius: 'var(--radius-sm)',
    padding: '3px 8px',
    cursor: 'pointer',
  },
  '.cm-inline-toolbar-btn:hover': {
    background: 'var(--color-surface-hover)',
    color: 'var(--color-accent)',
  },
  '&.cm-focused': { outline: 'none' },
});

/** CompletionSource автокомплита тегов (PROP-4, §8): инлайн `#tag` + frontmatter `tags:`-список из
 *  `list_tags`. Регекс-контекст в `tagCompletionQuery` (заголовок/code-span исключены). Монтируется в
 *  ЕДИНЫЙ autocompletion() рядом с wikilink/slash (EDIT-6 — два инстанса конфликтуют). */
function tagSource(fetchTags: () => Promise<string[]>): CompletionSource {
  return async (ctx: CompletionContext): Promise<CompletionResult | null> => {
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = ctx.state.sliceDoc(line.from, ctx.pos);
    const query = tagCompletionQuery(before);
    if (query === null) return null;
    const from = ctx.pos - query.length; // заменяем только набранный префикс (# / `[` сохраняются)
    const q = query.toLowerCase();
    const tags = await fetchTags();
    const options = tags
      .filter((t) => t.toLowerCase().includes(q))
      .slice(0, 50)
      .map((t) => ({ label: t, type: 'keyword' }));
    return { from, options, validFor: /^[\p{L}\p{N}_/-]*$/u };
  };
}

/** Колбэки редактора (через ref-геттеры — всегда актуальны без пересоздания view). */
export interface EditorCallbacks {
  /** Заметки по подстроке для автокомплита `[[…` (бэкенд-фильтр + лимит, #22). */
  fetchNotes: (query: string) => Promise<NoteRef[]>;
  /** Имена тегов vault для автокомплита `#tag` / `tags:` (PROP-4, плоский `list_tags`). */
  fetchTags: () => Promise<string[]>;
  getOpenLink: () => ((target: string) => void) | undefined;
}

/** Полный набор расширений source-mode редактора Nexus. */
export function nexusExtensions(cb: EditorCallbacks): Extension[] {
  return [
    markdown(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    decorationPlugin,
    wikilinkClick(cb.getOpenLink),
    // EDIT-6/PROP-4: ЕДИНЫЙ autocompletion() с источниками wikilink (`[[…`), slash (`/…`), tag (`#…`).
    // Контексты взаимоисключающие (по regex); монтировать два autocompletion() нельзя (конфликт конфигов).
    autocompletion({ override: [wikilinkSource(cb.fetchNotes), tagSource(cb.fetchTags), slashSource()] }),
    editorTheme,
    EditorView.lineWrapping,
  ];
}
