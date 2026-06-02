import { describe, expect, it } from 'vitest';

import type { ChatStreamEvent } from '../tauri-api';
import { searchContent, streamChat } from './vault';

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

describe('mock streamChat (контракт Ф1-7)', () => {
  it('эмитит sources → токены → done в правильном порядке', async () => {
    const events: ChatStreamEvent[] = [];
    await new Promise<void>((resolve) => {
      streamChat('Roadmap', (e) => {
        events.push(e);
        if (e.type === 'done' || e.type === 'error') resolve();
      });
    });
    expect(events[0].type).toBe('sources');
    expect(events.at(-1)?.type).toBe('done');
    expect(events.some((e) => e.type === 'token')).toBe(true);
    const done = events.at(-1);
    if (done?.type === 'done') expect(done.full.length).toBeGreaterThan(0);
  });

  it('отмена прекращает поток до done', async () => {
    const events: ChatStreamEvent[] = [];
    const cancel = streamChat('Roadmap', (e) => events.push(e));
    cancel(); // сразу отменяем
    await new Promise((r) => setTimeout(r, 80));
    expect(events.some((e) => e.type === 'done')).toBe(false);
  });
});
