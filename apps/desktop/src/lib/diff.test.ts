import { describe, expect, it } from 'vitest';
import { diffStat, lineDiff } from './diff';

describe('lineDiff (SAFE-6)', () => {
  it('одинаковый текст → всё same', () => {
    const d = lineDiff('a\nb\nc', 'a\nb\nc');
    expect(d.every((x) => x.type === 'same')).toBe(true);
  });

  it('замена строки = del старой + add новой, общие — same', () => {
    expect(lineDiff('a\nb\nc', 'a\nx\nc')).toEqual([
      { type: 'same', text: 'a' },
      { type: 'del', text: 'b' },
      { type: 'add', text: 'x' },
      { type: 'same', text: 'c' },
    ]);
  });

  it('diffStat считает добавленные и удалённые', () => {
    expect(diffStat(lineDiff('a\nb', 'a\nb\nc\nd'))).toEqual({ added: 2, removed: 0 });
    expect(diffStat(lineDiff('a\nb\nc', 'a'))).toEqual({ added: 0, removed: 2 });
  });
});
