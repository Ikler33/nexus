import { describe, expect, it } from 'vitest';

import { CITE_SCHEME, splitCitations } from './remarkCitations';

describe('splitCitations (AIP-2)', () => {
  it('одиночная сноска [1] → link-узел схемы nexus-cite', () => {
    const parts = splitCitations('как тут [1] видно');
    expect(parts).toEqual([
      { type: 'text', value: 'как тут ' },
      { type: 'link', url: `${CITE_SCHEME}1`, title: null, children: [{ type: 'text', value: '[1]' }] },
      { type: 'text', value: ' видно' },
    ]);
  });

  it('несколько сносок подряд и вперемешку с текстом', () => {
    const parts = splitCitations('a [1] b [23] c');
    const links = parts.filter((p) => p.type === 'link');
    expect(links.map((l) => (l.type === 'link' ? l.url : ''))).toEqual([
      `${CITE_SCHEME}1`,
      `${CITE_SCHEME}23`,
    ]);
  });

  it('нет сносок → один text-узел с исходным значением', () => {
    expect(splitCitations('обычный текст без сносок')).toEqual([
      { type: 'text', value: 'обычный текст без сносок' },
    ]);
  });

  it('1–3 цифры; 4+ цифр НЕ сноска', () => {
    expect(splitCitations('[999]').some((p) => p.type === 'link')).toBe(true);
    expect(splitCitations('[1234]').every((p) => p.type === 'text')).toBe(true);
  });

  it('пустые/буквенные скобки не считаются сносками', () => {
    expect(splitCitations('[] и [abc] и [x1]').every((p) => p.type === 'text')).toBe(true);
  });
});
