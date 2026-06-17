import type { Paragraph, Root } from 'mdast';
import { describe, expect, it } from 'vitest';

import { remarkComments } from './remarkComments';

/** Хелпер: дерево с одним абзацем-текстом. */
function doc(text: string): Root {
  return { type: 'root', children: [{ type: 'paragraph', children: [{ type: 'text', value: text }] }] };
}
function firstText(tree: Root): string | undefined {
  const p = tree.children[0] as Paragraph | undefined;
  const c = p?.children?.[0];
  return c && c.type === 'text' ? c.value : undefined;
}

describe('remarkComments', () => {
  it('инлайн %%c%% вырезается из текста', () => {
    const tree = doc('до %%секрет%% после');
    remarkComments()(tree);
    expect(firstText(tree)).toBe('до  после');
  });

  it('абзац целиком из комментария — удаляется', () => {
    const tree = doc('%%только коммент%%');
    remarkComments()(tree);
    expect(tree.children).toHaveLength(0);
  });

  it('блок %%\\n…\\n%% (один абзац, мягкие переносы) — удаляется', () => {
    const tree = doc('%%\nстрока 1\nстрока 2\n%%');
    remarkComments()(tree);
    expect(tree.children).toHaveLength(0);
  });

  it('неполный %% без пары — НЕ трогаем (не съедаем остаток)', () => {
    const tree = doc('текст %% без закрывашки и дальше');
    remarkComments()(tree);
    expect(firstText(tree)).toBe('текст %% без закрывашки и дальше');
  });

  it('два комментария в строке', () => {
    const tree = doc('a %%x%% b %%y%% c');
    remarkComments()(tree);
    expect(firstText(tree)).toBe('a  b  c');
  });

  it('текст без %% не трогается', () => {
    const tree = doc('обычный 50% текст');
    remarkComments()(tree);
    expect(firstText(tree)).toBe('обычный 50% текст');
  });

  it('inlineCode не затрагивается (узел не text)', () => {
    const tree: Root = {
      type: 'root',
      children: [
        {
          type: 'paragraph',
          children: [
            { type: 'text', value: 'код ' },
            { type: 'inlineCode', value: '%%not a comment%%' },
          ],
        },
      ],
    };
    remarkComments()(tree);
    const p = tree.children[0] as Paragraph;
    expect(p.children[1]).toEqual({ type: 'inlineCode', value: '%%not a comment%%' });
  });
});
