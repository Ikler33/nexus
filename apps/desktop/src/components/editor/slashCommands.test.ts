import { type CompletionResult, CompletionContext } from '@codemirror/autocomplete';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { isTaskLine } from '../../lib/editor/format';
import { useInlineAIStore } from '../../stores/inlineAI';
import { useUIStore } from '../../stores/ui';
import { useWorkspaceStore } from '../../stores/workspace';
import { SLASH_ITEMS, slashSource } from './slashCommands';

/** Прогон CompletionSource на тексте с курсором в конце (если pos не задан). */
function complete(doc: string, pos = doc.length): CompletionResult | null {
  const state = EditorState.create({ doc, selection: { anchor: pos } });
  return slashSource()(new CompletionContext(state, pos, false)) as CompletionResult | null;
}

/** Применяет slash-пункт `id` к документу `doc`: триггер `[idxOf('/'), doc.length]`. Возвращает текст. */
function applyItem(id: string, doc: string): string {
  const view = new EditorView({ state: EditorState.create({ doc, selection: { anchor: doc.length } }) });
  const item = SLASH_ITEMS.find((i) => i.id === id);
  if (!item) throw new Error(`no slash item ${id}`);
  item.apply(view, doc.indexOf('/'), doc.length);
  const out = view.state.doc.toString();
  view.destroy();
  return out;
}

afterEach(() => {
  vi.useRealTimers();
  useUIStore.setState({ templatesOpen: false });
});

describe('slashSource (EDIT-6) — триггер', () => {
  it('срабатывает на «/» в начале строки, from указывает на слэш', () => {
    const r = complete('/h1');
    expect(r).not.toBeNull();
    expect(r!.from).toBe(0);
    expect(r!.options.length).toBeGreaterThan(0);
  });

  it('срабатывает на «/» после пробела (пустой запрос → все пункты)', () => {
    const r = complete('заметка /');
    expect(r).not.toBeNull();
    expect(r!.from).toBe(8); // позиция '/'
    expect(r!.options).toHaveLength(SLASH_ITEMS.length);
  });

  it('НЕ срабатывает в середине слова (a/b) и в дроби (1/2)', () => {
    expect(complete('a/b')).toBeNull();
    expect(complete('1/2')).toBeNull();
  });

  it('НЕ срабатывает внутри незакрытого [[wikilink (там работает wikilink-источник)', () => {
    expect(complete('[[note /')).toBeNull();
  });

  // Инвариант взаимоисключения wikilink↔slash (на него опирается единый autocompletion с 2 источниками):
  // внутри открытого [[ — slash молчит; после ЗАКРЫТОГО [[…]] — slash снова работает.
  it('взаимоисключение: открытый [[ глушит slash, закрытый [[…]] — нет', () => {
    expect(complete('[[ /x')).toBeNull(); // открытый wikilink + пробел + слэш → slash bail
    expect(complete('[[Заметка]] /h')).not.toBeNull(); // закрытый → slash снова активен
  });

  it('фильтрует реестр по запросу (по id и по label)', () => {
    const r = complete('/tas');
    expect(r).not.toBeNull();
    expect(r!.options.map((o) => o.label).join(' ')).toMatch(/task|задач/i);
    expect(complete('/оглавление-которого-нет')).toBeNull(); // нет совпадений → закрыть попап
  });
});

describe('slashSource (EDIT-6) — вставки apply', () => {
  it('заголовки вставляют markdown-префикс в начало строки', () => {
    expect(applyItem('h1', '/h1')).toBe('# ');
    expect(applyItem('h2', '/h2')).toBe('## ');
    expect(applyItem('h3', '/h3')).toBe('### ');
  });

  it('списки/таск вставляют корректные маркеры; таск подпадает под isTaskLine', () => {
    expect(applyItem('bullet', '/bul')).toBe('- ');
    expect(applyItem('numbered', '/num')).toBe('1. ');
    const task = applyItem('task', '/task');
    expect(task).toBe('- [ ] ');
    expect(isTaskLine(task)).toBe(true); // кликабелен в превью (EDIT-5), продолжается по Enter (EDIT-3)
  });

  it('callout и разделитель', () => {
    expect(applyItem('callout', '/call')).toBe('> [!note] ');
    expect(applyItem('hr', '/hr')).toBe('---');
  });

  it('блочный префикс сохраняет текст, что был до слэша на строке', () => {
    expect(applyItem('h1', 'идея /h1')).toBe('# идея ');
  });

  it('дата вставляет dateStamp текущего дня (формат YYYY-MM-DD)', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 5, 14, 9, 0, 0));
    expect(applyItem('date', '/date')).toBe('2026-06-14');
  });

  it('ссылка вставляет []() с курсором в тексте', () => {
    expect(applyItem('link', '/link')).toBe('[]()');
  });

  it('таблица вставляет markdown-таблицу (snippet с плейсхолдерами)', () => {
    const out = applyItem('table', '/table');
    expect(out).toContain('| --- | --- |');
    expect(out.startsWith('| ')).toBe(true);
  });

  it('«из шаблона» убирает триггер и открывает TemplatePicker', () => {
    useUIStore.setState({ templatesOpen: false });
    const out = applyItem('template', '/tmpl');
    expect(out).toBe(''); // триггер удалён
    expect(useUIStore.getState().templatesOpen).toBe(true);
  });

  it('«/ai» убирает триггер и открывает InlineAI prompt-box в активной группе', () => {
    useInlineAIStore.setState({ openGroupId: null });
    useWorkspaceStore.setState({ activeGroupId: 'g-test' });
    const out = applyItem('ai', '/ai');
    expect(out).toBe(''); // триггер удалён
    expect(useInlineAIStore.getState().openGroupId).toBe('g-test');
  });
});
