import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { describe, expect, it } from 'vitest';
import { insertLink, parseTasks, toggleTask, toggleTaskAtLine, toggleWrap } from './format';

function mkView(doc: string, from: number, to = from): EditorView {
  return new EditorView({
    state: EditorState.create({ doc, selection: { anchor: from, head: to } }),
  });
}
const sel = (v: EditorView) => v.state.sliceDoc(v.state.selection.main.from, v.state.selection.main.to);

describe('toggleWrap (EDIT-1)', () => {
  it('оборачивает выделение маркером, выделение остаётся на тексте', () => {
    const v = mkView('bold text', 0, 4); // выделено «bold»
    toggleWrap(v, '**');
    expect(v.state.doc.toString()).toBe('**bold** text');
    expect(sel(v)).toBe('bold');
    v.destroy();
  });

  it('снимает обрамление, если выделение уже обёрнуто', () => {
    const v = mkView('**bold** text', 2, 6); // «bold» внутри ** **
    toggleWrap(v, '**');
    expect(v.state.doc.toString()).toBe('bold text');
    expect(sel(v)).toBe('bold');
    v.destroy();
  });

  it('пустое выделение — вставляет пару маркеров, курсор между ними', () => {
    const v = mkView('ab', 1, 1);
    toggleWrap(v, '*');
    expect(v.state.doc.toString()).toBe('a**b');
    expect(v.state.selection.main.empty).toBe(true);
    expect(v.state.selection.main.head).toBe(2); // a*|*b
    v.destroy();
  });

  it('курсив одиночным маркером работает так же', () => {
    const v = mkView('x', 0, 1);
    toggleWrap(v, '*');
    expect(v.state.doc.toString()).toBe('*x*');
    v.destroy();
  });

  // Регресс на находку ревью: курсив поверх жирного ДОБАВЛЯЕТ слой, а не снимает внешний.
  it('курсив на выделении внутри **bold** добавляет слой (не снимает жирный)', () => {
    const v = mkView('**bold**', 2, 6); // «bold» внутри ** ** ; маркер * совпал бы с внутренними
    toggleWrap(v, '*');
    expect(v.state.doc.toString()).toBe('***bold***'); // жирный цел, добавлен курсив
    v.destroy();
  });

  it('жирный на выделении внутри ***x*** (тройной прогон) оборачивает, не ломает', () => {
    const v = mkView('***x***', 3, 4); // «x» внутри ***...***
    toggleWrap(v, '**');
    expect(v.state.doc.toString()).toBe('*****x*****'); // ** добавлены вокруг x
    v.destroy();
  });

  it('частичное обрамление (маркер только слева) — оборачивает, текст цел', () => {
    const v = mkView('**bold text', 2, 6); // «bold», слева ** , справа пробел
    toggleWrap(v, '**');
    expect(v.state.doc.toString()).toBe('****bold** text');
    v.destroy();
  });
});

describe('toggleTask (EDIT-2)', () => {
  it('обычная строка → таск', () => {
    const v = mkView('купить молоко', 3, 3);
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('- [ ] купить молоко');
    v.destroy();
  });

  it('буллет → таск (нормализует маркер)', () => {
    const v = mkView('* пункт', 2, 2);
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('- [ ] пункт');
    v.destroy();
  });

  it('таск незавершённый → завершённый и обратно', () => {
    const v = mkView('- [ ] дело', 6, 6);
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('- [x] дело');
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('- [ ] дело');
    v.destroy();
  });

  it('сохраняет отступ', () => {
    const v = mkView('    - [ ] вложенный', 12, 12);
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('    - [x] вложенный');
    v.destroy();
  });

  it('пустая строка → пустой таск', () => {
    const v = mkView('', 0, 0);
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('- [ ] ');
    v.destroy();
  });

  it('мультистрочное выделение — каждая строка независимо', () => {
    const v = mkView('a\n- [ ] b\nc', 0, 11); // всё выделено (3 строки)
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('- [ ] a\n- [x] b\n- [ ] c');
    v.destroy();
  });

  // Регресс на находку ревью: выделение, кончающееся в col 0 строки ниже, не цепляет её.
  it('выделение до начала строки ниже (col 0) не трогает лишнюю строку', () => {
    const v = mkView('a\nb\nc', 0, 2); // выделено «a\n», курсор в начале «b»
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('- [ ] a\nb\nc');
    v.destroy();
  });

  // Регресс на находку ревью: лишние пробелы в боксе нормализуются, а не дублируют чекбокс.
  it('таск с лишними пробелами в боксе нормализуется, не дублируется', () => {
    const v = mkView('- [  ] дело', 7, 7);
    toggleTask(v);
    expect(v.state.doc.toString()).toBe('- [x] дело');
    v.destroy();
  });
});

describe('insertLink (EDIT-4)', () => {
  it('пустое выделение → []() с курсором в тексте', () => {
    const v = mkView('', 0, 0);
    insertLink(v);
    expect(v.state.doc.toString()).toBe('[]()');
    expect(v.state.selection.main.head).toBe(1); // [|]
    v.destroy();
  });

  it('выделен текст → [текст]() с курсором в адресе', () => {
    const v = mkView('читай доку', 6, 10); // выделено «доку»
    insertLink(v);
    expect(v.state.doc.toString()).toBe('читай [доку]()');
    expect(v.state.selection.main.head).toBe(6 + 'доку'.length + 3); // [доку](| → сразу после `](`
    expect(v.state.sliceDoc(v.state.selection.main.head - 1, v.state.selection.main.head)).toBe('(');
    v.destroy();
  });

  it('выделен URL → [](url) с курсором в тексте', () => {
    const v = mkView('https://nexus.app', 0, 17);
    insertLink(v);
    expect(v.state.doc.toString()).toBe('[](https://nexus.app)');
    expect(v.state.selection.main.head).toBe(1); // [|](url)
    v.destroy();
  });

  it('выделен www-адрес тоже распознаётся как ссылка', () => {
    const v = mkView('www.example.com', 0, 15);
    insertLink(v);
    expect(v.state.doc.toString()).toBe('[](www.example.com)');
    v.destroy();
  });

  it('выделение с пробелом — не URL, идёт в текст', () => {
    const v = mkView('два слова', 0, 9);
    insertLink(v);
    expect(v.state.doc.toString()).toBe('[два слова]()');
    v.destroy();
  });

  // Регресс на находку ревью: голый префикс схемы — это текст, не адрес.
  it('голый www. без контента — текст, не адрес', () => {
    const v = mkView('www.', 0, 4);
    insertLink(v);
    expect(v.state.doc.toString()).toBe('[www.]()');
    v.destroy();
  });

  // Регресс на находку ревью: скобки в тексте экранируются, чтобы не рвать `[…]`.
  it('скобки в выделении экранируются в тексте ссылки', () => {
    const v = mkView('a]b', 0, 3);
    insertLink(v);
    expect(v.state.doc.toString()).toBe('[a\\]b]()');
    expect(v.state.sliceDoc(v.state.selection.main.head - 1, v.state.selection.main.head)).toBe('(');
    v.destroy();
  });
});

describe('toggleTaskAtLine (EDIT-5, клик в превью)', () => {
  it('отмечает незавершённый таск на указанной строке', () => {
    expect(toggleTaskAtLine('- [ ] дело', 1)).toBe('- [x] дело');
  });

  it('снимает отметку с завершённого (включая [X])', () => {
    expect(toggleTaskAtLine('- [x] дело', 1)).toBe('- [ ] дело');
    expect(toggleTaskAtLine('- [X] дело', 1)).toBe('- [ ] дело');
  });

  it('трогает только указанную строку в многострочном документе', () => {
    const doc = 'заголовок\n- [ ] a\n- [ ] b';
    expect(toggleTaskAtLine(doc, 3)).toBe('заголовок\n- [ ] a\n- [x] b');
  });

  it('нумерованный таск и отступ сохраняются', () => {
    expect(toggleTaskAtLine('1. [ ] x', 1)).toBe('1. [x] x');
    expect(toggleTaskAtLine('    - [ ] y', 1)).toBe('    - [x] y');
  });

  it('строка не таск → null (защита от дрейфа)', () => {
    expect(toggleTaskAtLine('обычный текст', 1)).toBeNull();
    expect(toggleTaskAtLine('- буллет без бокса', 1)).toBeNull();
  });

  it('номер строки вне диапазона → null', () => {
    expect(toggleTaskAtLine('- [ ] a', 0)).toBeNull();
    expect(toggleTaskAtLine('- [ ] a', 2)).toBeNull();
  });

  // Регресс на находку ревью: CRLF-строка тогглится и СОХРАНЯЕТ \r (не плодит смешанные переводы).
  it('CRLF: тоггл сохраняет перевод строки', () => {
    expect(toggleTaskAtLine('- [ ] a\r\n- [x] b\r', 1)).toBe('- [x] a\r\n- [x] b\r');
  });
});

describe('parseTasks (TASK-1, дашборд)', () => {
  it('извлекает задачи с 1-based номерами строк, состоянием и текстом', () => {
    const doc = 'купить молоко\n- [ ] позвонить\n- [x] оплатить';
    expect(parseTasks(doc)).toEqual([
      { line: 2, checked: false, text: 'позвонить' },
      { line: 3, checked: true, text: 'оплатить' },
    ]);
  });

  it('маркеры */+, нумерованные, отступ, [X]', () => {
    expect(parseTasks('* [ ] a')).toEqual([{ line: 1, checked: false, text: 'a' }]);
    expect(parseTasks('+ [X] b')).toEqual([{ line: 1, checked: true, text: 'b' }]);
    expect(parseTasks('1. [ ] c')).toEqual([{ line: 1, checked: false, text: 'c' }]);
    expect(parseTasks('42) [ ] d')).toEqual([{ line: 1, checked: false, text: 'd' }]);
    expect(parseTasks('    - [x] e')).toEqual([{ line: 1, checked: true, text: 'e' }]);
  });

  it('не-задачи игнорируются (буллет, текст, пустая, таск в цитате)', () => {
    expect(parseTasks('- буллет\nобычный текст\n\n> - [ ] в цитате')).toEqual([]);
  });

  it('пустой документ → []', () => {
    expect(parseTasks('')).toEqual([]);
  });

  // Регресс на находку ревью: CRLF-файлы (Windows). Rust .lines() срезает \r — фронт обязан совпасть.
  it('CRLF: задачи распознаются, текст без \\r', () => {
    expect(parseTasks('- [ ] a\r\n- [x] b\r')).toEqual([
      { line: 1, checked: false, text: 'a' },
      { line: 2, checked: true, text: 'b' },
    ]);
  });
});
