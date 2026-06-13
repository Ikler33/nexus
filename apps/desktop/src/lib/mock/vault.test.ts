import { describe, expect, it } from 'vitest';

import type { ChatStreamEvent } from '../tauri-api';
import {
  __seedVersion,
  fileHash,
  getFullGraph,
  getLocalGraph,
  listVersions,
  readFileMeta,
  readVersion,
  searchContent,
  streamChat,
  writeFile,
} from './vault';

describe('mock версии (SAFE-5/6)', () => {
  it('listVersions сортирует по времени убыв.; readVersion отдаёт контент', async () => {
    __seedVersion('Notes/V.md', 'старое');
    const ts2 = __seedVersion('Notes/V.md', 'новое');
    const list = await listVersions('Notes/V.md');
    expect(list.length).toBe(2);
    expect(list[0].ts).toBe(ts2); // новейший первым
    expect(await readVersion('Notes/V.md', ts2)).toBe('новое');
  });

  it('listVersions пусто для файла без истории', async () => {
    expect(await listVersions('Notes/Никогда.md')).toEqual([]);
  });
});

describe('mock content-hash (SAFE-2)', () => {
  it('writeFile возвращает хеш, равный fileHash после записи', async () => {
    const h = await writeFile('Notes/Hash.md', '# Привет');
    expect(h).toBeTruthy();
    expect(await fileHash('Notes/Hash.md')).toBe(h);
  });

  it('хеш различает контент и стабилен', async () => {
    const a = await writeFile('Notes/H1.md', 'один');
    const b = await writeFile('Notes/H2.md', 'два');
    expect(a).not.toBe(b);
    expect(await writeFile('Notes/H1.md', 'один')).toBe(a);
  });

  it('fileHash несуществующего → null', async () => {
    expect(await fileHash('Notes/НетТакого-xyz.md')).toBeNull();
  });

  it('readFileMeta отдаёт content + согласованный hash', async () => {
    await writeFile('Notes/Meta.md', '# Мета\n\nтело');
    const meta = await readFileMeta('Notes/Meta.md');
    expect(meta.content).toBe('# Мета\n\nтело');
    expect(meta.hash).toBe(await fileHash('Notes/Meta.md'));
  });
});

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
    const hits = await searchContent('проект план alpha', { limit: 2 });
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

describe('mock graph (контракт ADR-004 / AC-DOD-Ф3)', () => {
  it('единый граф: с большим лимитом отдаёт все узлы и не обрезан', async () => {
    const full = await getFullGraph(10_000);
    expect(full.nodes.length).toBe(full.totalFiles);
    expect(full.truncated).toBe(false);
    // Узлы рёбер — внутри множества узлов графа.
    const ids = new Set(full.nodes.map((n) => n.id));
    for (const e of full.edges) {
      expect(ids.has(e.source)).toBe(true);
      expect(ids.has(e.target)).toBe(true);
    }
  });

  it('единый граф: маленький лимит → truncated и не больше лимита узлов', async () => {
    const top = await getFullGraph(2);
    expect(top.nodes.length).toBeLessThanOrEqual(2);
    expect(top.truncated).toBe(true);
    expect(top.totalFiles).toBeGreaterThan(top.nodes.length);
  });

  it('локальный граф несуществующего центра → пусто', async () => {
    const g = await getLocalGraph('Нет.md', 2);
    expect(g.nodes).toEqual([]);
    expect(g.edges).toEqual([]);
  });
});
