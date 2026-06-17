import type { Root } from 'mdast';
import { describe, expect, it } from 'vitest';

import { extractFrontmatter, parseFrontmatterFields } from './frontmatter';
import { remarkFrontmatter } from './remarkFrontmatter';

describe('extractFrontmatter', () => {
  it('находит блок и строку закрывающего ---', () => {
    const fm = extractFrontmatter('---\ntitle: X\ntags: [a]\n---\n\n# Body');
    expect(fm).toEqual({ raw: 'title: X\ntags: [a]', endLine: 4 });
  });
  it('нет frontmatter → null', () => {
    expect(extractFrontmatter('# просто заголовок')).toBeNull();
  });
  it('незакрытый блок → null', () => {
    expect(extractFrontmatter('---\ntitle: X\n\n# Body')).toBeNull();
  });
  it('--- не в начале → null', () => {
    expect(extractFrontmatter('текст\n---\nk: v\n---')).toBeNull();
  });
});

describe('parseFrontmatterFields', () => {
  it('скаляр k: v', () => {
    expect(parseFrontmatterFields('title: Привет')).toEqual([{ key: 'title', values: ['Привет'] }]);
  });
  it('инлайн-список k: [a, b]', () => {
    expect(parseFrontmatterFields('tags: [work, idea]')).toEqual([
      { key: 'tags', values: ['work', 'idea'] },
    ]);
  });
  it('блок-список k:\\n  - a\\n  - b', () => {
    expect(parseFrontmatterFields('aliases:\n  - Один\n  - Два')).toEqual([
      { key: 'aliases', values: ['Один', 'Два'] },
    ]);
  });
  it('кавычки снимаются', () => {
    expect(parseFrontmatterFields('title: "В кавычках"')).toEqual([
      { key: 'title', values: ['В кавычках'] },
    ]);
  });
  it('несколько полей по порядку', () => {
    expect(parseFrontmatterFields('title: A\nstatus: doing').map((f) => f.key)).toEqual([
      'title',
      'status',
    ]);
  });
  it('пустые строки игнорируются', () => {
    expect(parseFrontmatterFields('\ntitle: A\n\n')).toEqual([{ key: 'title', values: ['A'] }]);
  });
});

describe('remarkFrontmatter (удаление без сдвига строк)', () => {
  // Строим дерево вручную с позициями, имитируя `---\nk: v\n---\n\n# H` (H на строке 5).
  function tree(): Root {
    return {
      type: 'root',
      children: [
        { type: 'thematicBreak', position: { start: { line: 1, column: 1, offset: 0 }, end: { line: 1, column: 4, offset: 3 } } },
        { type: 'heading', depth: 2, children: [{ type: 'text', value: 'k: v' }], position: { start: { line: 2, column: 1, offset: 4 }, end: { line: 3, column: 4, offset: 12 } } },
        { type: 'heading', depth: 1, children: [{ type: 'text', value: 'H' }], position: { start: { line: 5, column: 1, offset: 14 }, end: { line: 5, column: 4, offset: 17 } } },
      ],
    } as Root;
  }
  it('узлы frontmatter (строки ≤ endLine) удалены, тело сохранено с позициями', () => {
    const t = tree();
    const file = { toString: () => '---\nk: v\n---\n\n# H' };
    remarkFrontmatter()(t, file);
    expect(t.children).toHaveLength(1);
    const h = t.children[0] as { type: string; position?: { start: { line: number } } };
    expect(h.type).toBe('heading');
    expect(h.position?.start.line).toBe(5); // позиция тела НЕ сдвинута
  });
  it('нет frontmatter → дерево не трогается', () => {
    const t: Root = { type: 'root', children: [{ type: 'paragraph', children: [{ type: 'text', value: 'x' }] }] };
    remarkFrontmatter()(t, { toString: () => 'x' });
    expect(t.children).toHaveLength(1);
  });
});
