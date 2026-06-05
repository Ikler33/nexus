import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { afterEach, describe, expect, it } from 'vitest';

import { clearActiveEditorView, getActiveEditorView, setActiveEditorView } from './activeView';

afterEach(() => setActiveEditorView(null));

describe('activeView registry (IL-3)', () => {
  it('set/get возвращает зарегистрированный view; clear снимает только его', () => {
    const a = new EditorView({ state: EditorState.create({ doc: 'a' }) });
    const b = new EditorView({ state: EditorState.create({ doc: 'b' }) });

    setActiveEditorView(a);
    expect(getActiveEditorView()).toBe(a);

    // clear другого view не трогает активный
    clearActiveEditorView(b);
    expect(getActiveEditorView()).toBe(a);

    // clear активного — снимает
    clearActiveEditorView(a);
    expect(getActiveEditorView()).toBeNull();

    a.destroy();
    b.destroy();
  });
});
