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
  setFrontmatterField,
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

describe('mock setFrontmatterField (BOARD-1 — зеркало Rust-контракта)', () => {
  it('заменяет существующий ключ, сохраняя остальной YAML и тело', async () => {
    await writeFile('Tasks/A.md', '---\nstatus: todo\nproject: Alpha\n---\n# H\nтело\n');
    const { content } = await setFrontmatterField('Tasks/A.md', 'status', 'doing');
    expect(content).toBe('---\nstatus: doing\nproject: Alpha\n---\n# H\nтело\n');
  });

  it('добавляет отсутствующий ключ перед закрывающим ---', async () => {
    await writeFile('Tasks/B.md', '---\nstatus: todo\n---\nbody\n');
    const { content } = await setFrontmatterField('Tasks/B.md', 'priority', 'high');
    expect(content).toBe('---\nstatus: todo\npriority: high\n---\nbody\n');
  });

  it('создаёт frontmatter, если его нет; квотирует спецсимволы', async () => {
    await writeFile('Tasks/C.md', 'просто тело\n');
    const { content } = await setFrontmatterField('Tasks/C.md', 'status', 'todo');
    expect(content).toBe('---\nstatus: todo\n---\n\nпросто тело\n');
    await writeFile('Tasks/D.md', '---\nx: 1\n---\nb\n');
    const r = await setFrontmatterField('Tasks/D.md', 'status', 'a: b');
    expect(r.content).toContain('status: "a: b"');
  });

  it('незакрытый frontmatter → throw (как Err(Malformed))', async () => {
    await writeFile('Tasks/E.md', '---\nstatus: todo\nбез закрытия\n');
    await expect(setFrontmatterField('Tasks/E.md', 'status', 'x')).rejects.toThrow();
  });

  it('F4: дубль-ключ — правит ПОСЛЕДНЕЕ вхождение (last-key-wins)', async () => {
    await writeFile('Tasks/F.md', '---\nstatus: a\nstatus: b\n---\nbody\n');
    const { content } = await setFrontmatterField('Tasks/F.md', 'status', 'c');
    expect(content).toBe('---\nstatus: a\nstatus: c\n---\nbody\n');
  });

  it('F5: добавление ключа в CRLF-файл — новая строка тоже CRLF', async () => {
    await writeFile('Tasks/G.md', '---\r\nstatus: todo\r\n---\r\nbody\r\n');
    const { content } = await setFrontmatterField('Tasks/G.md', 'priority', 'high');
    expect(content).toBe('---\r\nstatus: todo\r\npriority: high\r\n---\r\nbody\r\n');
  });

  it('F3: невалидный ключ → throw (как Rust value_key)', async () => {
    await writeFile('Tasks/H.md', '---\nstatus: todo\n---\nbody\n');
    await expect(setFrontmatterField('Tasks/H.md', 'foo:bar', 'x')).rejects.toThrow();
    await expect(setFrontmatterField('Tasks/H.md', '', 'x')).rejects.toThrow();
  });

  it('F1/F2: значение без round-trip → throw (краевая кавычка / перевод строки)', async () => {
    await writeFile('Tasks/I.md', '---\nx: old\n---\nbody\n');
    await expect(setFrontmatterField('Tasks/I.md', 'status', 'say "hi"')).rejects.toThrow();
    await expect(setFrontmatterField('Tasks/I.md', 'status', 'a\nb')).rejects.toThrow();
    // Интерьерная кавычка (не на краю) — допустима.
    const { content } = await setFrontmatterField('Tasks/I.md', 'status', 'a "x" b');
    expect(content).toContain('status: a "x" b');
  });

  it('m8: целевой ключ — инлайн-список → throw, файл не тронут', async () => {
    const src = '---\nstatus: [a, b]\nproject: Alpha\n---\nтело\n';
    await writeFile('Tasks/J.md', src);
    await expect(setFrontmatterField('Tasks/J.md', 'status', 'doing')).rejects.toThrow();
    // Файл байт-в-байт цел (мок не мутировал CONTENT при отказе).
    expect((await readFileMeta('Tasks/J.md')).content).toBe(src);
  });

  it('m8: целевой ключ — блок-список → throw, файл не тронут', async () => {
    const src = '---\ntags:\n  - a\n  - b\nproject: Alpha\n---\nbody\n';
    await writeFile('Tasks/K.md', src);
    await expect(setFrontmatterField('Tasks/K.md', 'tags', 'x')).rejects.toThrow();
    expect((await readFileMeta('Tasks/K.md')).content).toBe(src);
  });

  it('m8: запись скаляра в ключ НАД чужим блок-списком не съедает чужой список', async () => {
    await writeFile('Tasks/L.md', '---\nstatus: todo\ntags:\n  - a\n  - b\n---\nbody\n');
    const { content } = await setFrontmatterField('Tasks/L.md', 'status', 'doing');
    expect(content).toBe('---\nstatus: doing\ntags:\n  - a\n  - b\n---\nbody\n');
  });

  it('m8: целевой ключ — блок-скаляр |/> → throw, файл не тронут', async () => {
    const src = '---\ndesc: |\n  l1\n  l2\nproject: A\n---\nbody\n';
    await writeFile('Tasks/M.md', src);
    await expect(setFrontmatterField('Tasks/M.md', 'desc', 'v')).rejects.toThrow();
    expect((await readFileMeta('Tasks/M.md')).content).toBe(src);
  });

  it('m8: целевой ключ — вложенный блок-маппинг → throw, файл не тронут', async () => {
    const src = '---\nnested:\n  sub: 1\nproject: Alpha\n---\nbody\n';
    await writeFile('Tasks/N.md', src);
    await expect(setFrontmatterField('Tasks/N.md', 'nested', 'v')).rejects.toThrow();
    expect((await readFileMeta('Tasks/N.md')).content).toBe(src);
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

  // P1-19: реципрокная пара (Roadmap [[Spec]] И Spec [[Roadmap]] — взаимные вики-ссылки в CONTENT)
  // даёт РОВНО ОДНО ненаправленное ребро, зеркаля бэкенд (dedup в канонические пары min/max).
  // Без дедупа было бы (a,b) И (b,a) → вдвое завышенная степень/размер/счётчик и двойная линия.
  it('реципрокная пара → ровно одно ненаправленное ребро (зеркало бэкенда)', async () => {
    const full = await getFullGraph(10_000);
    const idOf = (path: string) => full.nodes.find((n) => n.path === path)?.id;
    const roadmap = idOf('Projects/Roadmap.md');
    const spec = idOf('Projects/Alpha/Spec.md');
    expect(roadmap).toBeDefined();
    expect(spec).toBeDefined();

    // Число рёбер между неупорядоченной парой {roadmap, spec} — должно быть ровно одно.
    const between = full.edges.filter(
      (e) =>
        (e.source === roadmap && e.target === spec) ||
        (e.source === spec && e.target === roadmap),
    );
    expect(between).toHaveLength(1);

    // Ни одной дублирующей неупорядоченной пары во всём графе (контракт «одно ребро на пару»).
    const seen = new Set<string>();
    for (const e of full.edges) {
      const key = [e.source, e.target].sort((a, b) => a - b).join('|');
      expect(seen.has(key)).toBe(false);
      seen.add(key);
      expect(e.source).not.toBe(e.target); // self-loop не бывает
    }

    // Локальный граф из Roadmap — та же дедупликация (1 ребро на пару Roadmap↔Spec).
    const local = await getLocalGraph('Projects/Roadmap.md', 1);
    const lRoadmap = local.nodes.find((n) => n.path === 'Projects/Roadmap.md')?.id;
    const lSpec = local.nodes.find((n) => n.path === 'Projects/Alpha/Spec.md')?.id;
    const lBetween = local.edges.filter(
      (e) =>
        (e.source === lRoadmap && e.target === lSpec) ||
        (e.source === lSpec && e.target === lRoadmap),
    );
    expect(lBetween).toHaveLength(1);
  });
});
