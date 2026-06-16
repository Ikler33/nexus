import { describe, expect, it } from 'vitest';

import { forNote, inferType } from './properties';

describe('mock properties (PROP-2 — зеркало Rust-эвристики)', () => {
  it('inferType: порядок forced→bool→datetime→date→number→list→text', () => {
    expect(inferType('tags', 'что угодно')).toBe('tags');
    expect(inferType('done', 'Off')).toBe('checkbox'); // bool до number
    expect(inferType('ts', '2026-06-20T14:30')).toBe('datetime');
    expect(inferType('due', '2026-06-20')).toBe('date');
    expect(inferType('priority', '3')).toBe('number');
    expect(inferType('authors', '[a, b]')).toBe('list');
    expect(inferType('note', 'Привет, мир')).toBe('text'); // CSV-текст ≠ список
    expect(inferType('status', 'todo')).toBe('text');
  });

  it('forNote отдаёт скаляры с разрешённым типом', async () => {
    const props = await forNote();
    const byKey = Object.fromEntries(props.map((p) => [p.key, p.type]));
    expect(byKey.status).toBe('text');
    expect(byKey.due).toBe('date');
    expect(byKey.created).toBe('date');
  });
});
