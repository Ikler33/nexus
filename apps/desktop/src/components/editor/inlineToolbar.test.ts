import { EditorState } from '@codemirror/state';
import { describe, expect, it } from 'vitest';

import { ghostField, setGhost } from '../../lib/editor/inlineGhost';
import { selectionTooltips } from './inlineToolbar';

const stateWith = (doc: string, anchor: number, head: number) =>
  EditorState.create({ doc, selection: { anchor, head }, extensions: [ghostField] });

describe('inlineToolbar (IL-3, D4)', () => {
  it('пустое выделение → тулбара нет', () => {
    expect(selectionTooltips(stateWith('hello', 2, 2))).toHaveLength(0);
  });

  it('непустое выделение → один тулбар, заякорен на начало выделения', () => {
    const tips = selectionTooltips(stateWith('hello world', 0, 5));
    expect(tips).toHaveLength(1);
    expect(tips[0].pos).toBe(0);
    expect(tips[0].above).toBe(true);
  });

  it('активный ghost → тулбар скрыт (не поверх предложения)', () => {
    let s = stateWith('hello world', 0, 5);
    s = s.update({ effects: setGhost.of({ pos: 5, from: 0, to: 5 }) }).state;
    expect(selectionTooltips(s)).toHaveLength(0);
  });
});
