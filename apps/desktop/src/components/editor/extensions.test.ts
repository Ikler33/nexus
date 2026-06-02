import { describe, expect, it } from 'vitest';
import { normalizeTarget } from './extensions';

describe('normalizeTarget (Ф0-5)', () => {
  it('возвращает цель без изменений', () => {
    expect(normalizeTarget('Note')).toBe('Note');
    expect(normalizeTarget('Projects/Roadmap')).toBe('Projects/Roadmap');
  });

  it('срезает #heading и |alias и тримит', () => {
    expect(normalizeTarget('Note#Section')).toBe('Note');
    expect(normalizeTarget('Note#Section|Alias')).toBe('Note');
    expect(normalizeTarget('  Spaced Note  ')).toBe('Spaced Note');
  });
});
