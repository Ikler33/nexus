import { describe, expect, it } from 'vitest';

import {
  endpointId,
  kinSet,
  neighborSet,
  nodeRadius,
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

  it('nodeRadius: sqrt-шкала, клампится 5..28', () => {
    expect(nodeRadius(0)).toBe(5);
    expect(nodeRadius(1)).toBeCloseTo(9.2, 5);
    expect(nodeRadius(100)).toBe(28); // 5 + 10·4.2 = 47 → клампится в 28
  });

  it('endpointId: id из строки или из узла (до/после d3-init)', () => {
    const node = { id: 'x', title: 'X', path: 'x.md', deg: 0 } as GraphNodeDatum;
    expect(endpointId('y')).toBe('y');
    expect(endpointId(node)).toBe('x');
  });
});
