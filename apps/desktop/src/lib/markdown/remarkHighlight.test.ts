import type { Root, Strong } from 'mdast';
import { describe, expect, it } from 'vitest';

import { remarkHighlight, splitHighlights } from './remarkHighlight';

/** Хелпер: hName/children из mark-узла. */
function asMark(node: unknown): { hName?: string; text?: string } {
  const s = node as Strong & { data?: { hName?: string } };
  const child = s.children?.[0];
  return { hName: s.data?.hName, text: child && child.type === 'text' ? child.value : undefined };
}

describe('splitHighlights', () => {
  it('простое выделение', () => {
    const parts = splitHighlights('a ==важно== b');
    expect(parts).toHaveLength(3);
    expect(parts[0]).toEqual({ type: 'text', value: 'a ' });
    expect(asMark(parts[1])).toEqual({ hName: 'mark', text: 'важно' });
    expect(parts[2]).toEqual({ type: 'text', value: ' b' });
  });
  it('два выделения в строке (не жадный)', () => {
    const parts = splitHighlights('==a== и ==b==');
    expect(parts.filter((p) => (p as Strong).data?.hName === 'mark')).toHaveLength(2);
    expect(asMark(parts[0]).text).toBe('a');
    expect(asMark(parts[2]).text).toBe('b');
  });
  it('пробелы внутри допустимы', () => {
    expect(asMark(splitHighlights('==два слова==')[0]).text).toBe('два слова');
  });
  it('нет совпадения → один text-узел', () => {
    expect(splitHighlights('обычный текст')).toEqual([{ type: 'text', value: 'обычный текст' }]);
  });
  it('одиночный = не выделение', () => {
    expect(splitHighlights('a = b')).toEqual([{ type: 'text', value: 'a = b' }]);
  });
  it('=== / ==== не выделение (внутри нет не-= содержимого)', () => {
    expect(splitHighlights('===').every((p) => p.type === 'text')).toBe(true);
    expect(splitHighlights('====').every((p) => p.type === 'text')).toBe(true);
  });
  it('==   == (только пробелы) не выделение', () => {
    expect(splitHighlights('==   ==')).toEqual([{ type: 'text', value: '==   ==' }]);
  });
  it('инлайн-= внутри (a=b) не матчится (без = внутри)', () => {
    expect(splitHighlights('==a=b==')).toEqual([{ type: 'text', value: '==a=b==' }]);
  });
  it('пробельное содержимое отсеивается, реальное выделение после — остаётся', () => {
    const parts = splitHighlights('==  == ==ok==');
    expect(parts.filter((p) => (p as Strong).data?.hName === 'mark')).toHaveLength(1);
    expect(asMark(parts.find((p) => (p as Strong).data?.hName === 'mark')).text).toBe('ok');
  });
  // Перф-регрессия (находка ревью): длинная строка с открывашкой `==` и без закрывашки раньше давала
  // O(n²)-скан (UI-фриз). Один ленивый квантор → линейно: 100k символов должны обработаться мгновенно.
  it('длинная строка без закрывашки обрабатывается быстро (нет ReDoS O(n²))', () => {
    const big = '==' + 'a'.repeat(100_000);
    const start = performance.now();
    const parts = splitHighlights(big);
    const ms = performance.now() - start;
    expect(parts).toEqual([{ type: 'text', value: big }]); // нет пары → один text-узел
    expect(ms).toBeLessThan(200); // линейно: << 200 мс (двойной `*?` давал ~2 с)
  });
});

describe('remarkHighlight (дерево)', () => {
  it('заменяет text на text/mark', () => {
    const tree: Root = {
      type: 'root',
      children: [{ type: 'paragraph', children: [{ type: 'text', value: 'see ==X== ok' }] }],
    };
    remarkHighlight()(tree);
    const para = tree.children[0] as { children: unknown[] };
    expect(para.children).toHaveLength(3);
    expect(asMark(para.children[1])).toEqual({ hName: 'mark', text: 'X' });
  });
  it('текст без == не трогается', () => {
    const tree: Root = {
      type: 'root',
      children: [{ type: 'paragraph', children: [{ type: 'text', value: 'plain' }] }],
    };
    remarkHighlight()(tree);
    expect((tree.children[0] as { children: unknown[] }).children).toEqual([{ type: 'text', value: 'plain' }]);
  });
});
