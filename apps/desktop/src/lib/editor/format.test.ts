import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { describe, expect, it } from 'vitest';
import { toggleTask, toggleWrap } from './format';

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
