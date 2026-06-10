import { describe, expect, it } from 'vitest';

import {
  endpointId,
  kinSet,
  neighborSet,
  nodeColor,
  nodeRadius,
  tagHue,
  topTags,
  type EdgeIds,
  type GraphNodeDatum,
} from './graph-sim';

const chain: EdgeIds[] = [
  { source: 'a', target: 'b' },
  { source: 'b', target: 'c' },
  { source: 'c', target: 'd' },
];

describe('graph-sim (помощники подсветки/размера)', () => {
  it('neighborSet: фокус + прямые соседи (или null)', () => {
    expect(neighborSet(chain, null)).toBeNull();
    expect(neighborSet(chain, 'b')).toEqual(new Set(['b', 'a', 'c']));
    expect(neighborSet(chain, 'a')).toEqual(new Set(['a', 'b']));
  });

  it('kinSet: соседи активной ноты без неё самой', () => {
    expect(kinSet(chain, 'b')).toEqual(new Set(['a', 'c']));
    expect(kinSet(chain, 'a')).toEqual(new Set(['b']));
    expect(kinSet(chain, null)).toEqual(new Set());
  });

  it('nodeRadius: формула макета — сирота-точка 3.5, дальше 5.5..15', () => {
    expect(nodeRadius(0)).toBe(3.5); // сирота — точка гало
    expect(nodeRadius(1)).toBeCloseTo(6.6, 5); // 5 + 1·1.6
    expect(nodeRadius(100)).toBe(15); // клампится в 15 (хаб)
  });

  it('endpointId: id из строки или из узла (до/после d3-init)', () => {
    const node = { id: 'x', title: 'X', path: 'x.md', deg: 0, tags: [] } as GraphNodeDatum;
    expect(endpointId('y')).toBe('y');
    expect(endpointId(node)).toBe('x');
  });

  it('tagHue: детерминирован, в 0..359, разные теги — разные оттенки', () => {
    expect(tagHue('demo')).toBe(tagHue('demo'));
    for (const t of ['demo', 'docs', 'planning', 'идеи']) {
      const h = tagHue(t);
      expect(h).toBeGreaterThanOrEqual(0);
      expect(h).toBeLessThan(360);
    }
    expect(tagHue('demo')).not.toBe(tagHue('docs'));
  });

  it('nodeColor: oklch по первому тегу; без тегов — null (фолбэк CSS)', () => {
    expect(nodeColor([])).toBeNull();
    expect(nodeColor(['demo', 'docs'])).toBe(
      `oklch(var(--g-tag-l, 0.55) var(--g-tag-c, 0.12) ${tagHue('demo')})`,
    );
  });

  it('topTags: по частоте, ничья по алфавиту, обрезка по лимиту', () => {
    const nodes = [
      { tags: ['b', 'a'] },
      { tags: ['b'] },
      { tags: ['c'] },
      { tags: [] },
    ];
    expect(topTags(nodes, 8)).toEqual(['b', 'a', 'c']);
    expect(topTags(nodes, 2)).toEqual(['b', 'a']);
    expect(topTags([], 8)).toEqual([]);
  });
});
