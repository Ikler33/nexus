import { describe, expect, it } from 'vitest';

import { searchContent } from './vault';

describe('mock searchContent (контракт Ф1-6)', () => {
  it('пустой запрос → пусто', async () => {
    expect(await searchContent('   ')).toEqual([]);
    expect(await searchContent('')).toEqual([]);
  });

  it('находит файл по слову из тела, со сниппетом и score', async () => {
    const hits = await searchContent('Roadmap');
    expect(hits.length).toBeGreaterThan(0);
    const top = hits[0];
    expect(top.path).toBeTruthy();
    expect(top.snippet.length).toBeGreaterThan(0);
    expect(top.score).toBeGreaterThan(0);
  });

  it('сортирует по score↓ и режет по limit', async () => {
    const hits = await searchContent('проект план alpha', 2);
    expect(hits.length).toBeLessThanOrEqual(2);
    for (let i = 1; i < hits.length; i++) {
      expect(hits[i - 1].score).toBeGreaterThanOrEqual(hits[i].score);
    }
  });

  it('нет совпадений → пусто', async () => {
    expect(await searchContent('zzzнетничего')).toEqual([]);
  });
});
