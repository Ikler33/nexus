import { markdown, markdownKeymap } from '@codemirror/lang-markdown';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { describe, expect, it } from 'vitest';

// EDIT-3: умное продолжение списков опирается на штатные команды @codemirror/lang-markdown,
// подключённые в Editor.tsx через `Prec.high(keymap.of(markdownKeymap))`. Тестируем РОВНО те
// команды, что в keymap (а не предполагаемые имена экспортов) в нашем окружении/версии.
const continueMarkup = markdownKeymap.find((b) => b.key === 'Enter')!.run!;
const deleteMarkup = markdownKeymap.find((b) => b.key === 'Backspace')!.run!;

/** View с markdown-языком (нужен синтакс-дерево для распознавания списков) и курсором в pos. */
function mkView(doc: string, pos: number): EditorView {
  return new EditorView({
    state: EditorState.create({ doc, selection: { anchor: pos }, extensions: [markdown()] }),
  });
}

describe('list continuation (EDIT-3, markdownKeymap)', () => {
  it('Enter в конце пункта `- ` продолжает список', () => {
    const v = mkView('- молоко', 8); // курсор в конце строки
    const handled = continueMarkup(v);
    expect(handled).toBe(true);
    expect(v.state.doc.toString()).toBe('- молоко\n- ');
    v.destroy();
  });

  it('Enter в чекбоксе `- [ ]` продолжает свежим незачёркнутым таском', () => {
    const v = mkView('- [ ] дело', 10);
    expect(continueMarkup(v)).toBe(true);
    expect(v.state.doc.toString()).toBe('- [ ] дело\n- [ ] ');
    v.destroy();
  });

  it('Enter в нумерованном списке инкрементирует номер', () => {
    const v = mkView('1. один', 7);
    expect(continueMarkup(v)).toBe(true);
    expect(v.state.doc.toString()).toBe('1. один\n2. ');
    v.destroy();
  });

  it('Enter на ПУСТОМ пункте выходит из списка (маркер убирается)', () => {
    const v = mkView('- ', 2); // пустой буллет
    expect(continueMarkup(v)).toBe(true);
    expect(v.state.doc.toString()).toBe('');
    v.destroy();
  });

  it('Enter на обычной строке не перехватывается (false → обычная вставка)', () => {
    const v = mkView('просто текст', 12);
    expect(continueMarkup(v)).toBe(false);
    expect(v.state.doc.toString()).toBe('просто текст'); // команда не меняет документ
    v.destroy();
  });

  it('Backspace в начале текста пункта стирает маркер', () => {
    const v = mkView('- дело', 2); // курсор сразу после «- »
    expect(deleteMarkup(v)).toBe(true);
    expect(v.state.doc.toString()).toBe('дело');
    v.destroy();
  });
});
