import { describe, expect, it } from 'vitest';

import { makeSlugger, slugify } from './slug';

describe('slugify', () => {
  it('пробелы → дефис, нижний регистр', () => {
    expect(slugify('Hello World')).toBe('hello-world');
  });
  it('пунктуация выкидывается', () => {
    expect(slugify('What? Now! (really)')).toBe('what-now-really');
  });
  it('Unicode (кириллица) сохраняется', () => {
    expect(slugify('Раздел Первый')).toBe('раздел-первый');
  });
  it('повторные дефисы схлопываются, края обрезаются', () => {
    expect(slugify('  a — b  ')).toBe('a-b');
  });
  it('цифры остаются', () => {
    expect(slugify('Step 2: go')).toBe('step-2-go');
  });
});

describe('makeSlugger (дедуп в документе)', () => {
  it('повторы получают -1, -2', () => {
    const s = makeSlugger();
    expect(s('Intro')).toBe('intro');
    expect(s('Intro')).toBe('intro-1');
    expect(s('Intro')).toBe('intro-2');
  });
  it('разные базы независимы', () => {
    const s = makeSlugger();
    expect(s('A')).toBe('a');
    expect(s('B')).toBe('b');
    expect(s('A')).toBe('a-1');
  });
  it('пустой slug → section', () => {
    expect(makeSlugger()('???')).toBe('section');
  });
});
