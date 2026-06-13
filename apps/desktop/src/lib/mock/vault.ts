import type {
  BacklinkEntry,
  ChatStreamEvent,
  Contradiction,
  Digest,
  FileEntry,
  FullGraph,
  GoalEntry,
  GraphData,
  GraphEdge,
  InlineMode,
  InlineStreamEvent,
  LinkSuggestion,
  NoteRef,
  SearchHit,
  VaultInfo,
} from '../tauri-api';

/**
 * Фейковый vault для браузерного превью и тестов (DESIGN §0): фронт работает на тех же
 * контрактах `tauriApi`, что и реальный бэкенд, не дожидаясь Rust.
 */

function basename(path: string): string {
  const i = path.lastIndexOf('/');
  return i >= 0 ? path.slice(i + 1) : path;
}

function dir(path: string, hasChildren: boolean): FileEntry {
  return { name: basename(path), path, isDir: true, hasChildren, sizeBytes: 0 };
}

function file(path: string, sizeBytes: number): FileEntry {
  return { name: basename(path), path, isDir: false, hasChildren: false, sizeBytes };
}

/** Каталог → его непосредственные дети (ленивая модель, как у Rust `list_dir`). */
const TREE: Record<string, FileEntry[]> = {
  '': [
    dir('Projects', true),
    dir('Notes', true),
    dir('Empty', false),
    file('README.md', 1200),
    file('Inbox.md', 340),
  ],
  Projects: [dir('Projects/Alpha', true), file('Projects/Roadmap.md', 800)],
  'Projects/Alpha': [
    file('Projects/Alpha/Spec.md', 2400),
    file('Projects/Alpha/Notes.md', 560),
  ],
  Notes: [
    file('Notes/Idea.md', 210),
    file('Notes/Meeting.md', 980),
    file('Notes/Scratch.md', 120),
    file('Notes/Цитаты.md', 300),
    file('Notes/diagram.png', 4096),
  ],
  Empty: [],
};

/** Содержимое мок-файлов (правки в превью держим в этой мапе). */
const CONTENT: Record<string, string> = {
  'README.md': '# Mock Vault\n\nДемо-хранилище для превью без Tauri.\n\nСвязи: [[Inbox]] и [[Projects/Roadmap]].\nТеги: #demo #docs\n',
  'Inbox.md': '# Inbox\n\nБыстрые заметки. Ссылка на [[README]].\n',
  'Projects/Roadmap.md': '# Roadmap\n\nПлан проекта Alpha. См. [[Projects/Alpha/Spec]].\n#planning\n',
  'Projects/Alpha/Spec.md': '# Alpha Spec\n\nСпецификация. Обратно к [[Projects/Roadmap]].\n',
  'Projects/Alpha/Notes.md': '# Alpha Notes\n\nЗаметки по Alpha. #alpha\n',
  'Notes/Idea.md': '# Idea\n\nИдея с тегом #idea и ссылкой [[Notes/Meeting]].\n',
  'Notes/Meeting.md': '# Meeting\n\nПротокол встречи.\n',
  // Сироты без ссылок — гало глобального графа в превью (как halo-точки макета graph.jsx).
  'Notes/Scratch.md': '# Scratch\n\nЧерновик без связей.\n',
  'Notes/Цитаты.md': '# Цитаты\n\nКоллекция цитат.\n',
};

export async function openVault(path: string): Promise<VaultInfo> {
  return { root: path || '/mock/vault', name: 'Mock Vault' };
}

export async function listDir(dirPath: string): Promise<FileEntry[]> {
  return TREE[dirPath] ?? [];
}

export async function readFile(path: string): Promise<string> {
  return CONTENT[path] ?? `# ${basename(path)}\n\n(пустой мок-файл)\n`;
}

/** Детерминированный контент-хеш для мока (FNV-1a, hex). Бэкенд использует blake3; моку важна лишь
 *  стабильность и различение контента (baseHash буфера в тестах), не совпадение с blake3. */
function mockHash(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, '0');
}

export async function readFileMeta(path: string): Promise<{ content: string; hash: string }> {
  const content = await readFile(path);
  return { content, hash: mockHash(content) };
}

export async function fileHash(path: string): Promise<string | null> {
  return CONTENT[path] !== undefined ? mockHash(CONTENT[path]) : null;
}

export async function writeFile(path: string, content: string): Promise<string> {
  CONTENT[path] = content;
  return mockHash(content);
}

// История версий (SAFE-5/6): мок держит снапшоты в памяти, чтобы UI можно было гонять в браузере.
const VERSIONS: Record<string, { ts: number; content: string }[]> = {};
let versionSeq = 1_700_000_000_000;

export async function listVersions(path: string): Promise<{ ts: number; size: number }[]> {
  return (VERSIONS[path] ?? [])
    .map((v) => ({ ts: v.ts, size: v.content.length }))
    .sort((a, b) => b.ts - a.ts);
}

export async function readVersion(path: string, ts: number): Promise<string> {
  const v = (VERSIONS[path] ?? []).find((x) => x.ts === ts);
  return v?.content ?? '';
}

/** Только для мок-режима/тестов: засеять снапшот версии. */
export function __seedVersion(path: string, content: string): number {
  const ts = versionSeq++;
  (VERSIONS[path] ??= []).push({ ts, content });
  return ts;
}

function lineContext(content: string, idx: number): string {
  const start = content.lastIndexOf('\n', idx) + 1;
  const end = content.indexOf('\n', idx);
  return content.slice(start, end === -1 ? content.length : end).trim();
}

export async function getBacklinks(path: string): Promise<BacklinkEntry[]> {
  const noExt = path.endsWith('.md') ? path.slice(0, -3) : path;
  const base = basename(noExt);
  const out: BacklinkEntry[] = [];
  for (const [src, content] of Object.entries(CONTENT)) {
    if (src === path) continue;
    const re = /\[\[([^\]\n]+?)\]\]/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(content)) !== null) {
      const target = m[1].split('|')[0].split('#')[0].trim();
      if (target === path || target === noExt || target === base) {
        out.push({
          sourcePath: src,
          sourceTitle: null,
          context: lineContext(content, m.index),
          lineNumber: content.slice(0, m.index).split('\n').length,
        });
      }
    }
  }
  return out.sort((a, b) => a.sourcePath.localeCompare(b.sourcePath));
}

export async function searchVault(query: string): Promise<NoteRef[]> {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const notes = await listNotes();
  return notes.filter((n) => {
    if (n.path.toLowerCase().includes(q)) return true;
    const content = (CONTENT[n.path] ?? '').toLowerCase();
    return content.includes(`#${q}`); // совпадение по тегу
  });
}

export async function searchContent(
  query: string,
  opts?: { limit?: number; folder?: string; tag?: string; center?: string },
): Promise<SearchHit[]> {
  const limit = opts?.limit ?? 10;
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const terms = q.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  if (terms.length === 0) return [];

  const hits: SearchHit[] = [];
  for (const [path, content] of Object.entries(CONTENT)) {
    if (opts?.folder && path !== opts.folder && !path.startsWith(`${opts.folder}/`)) continue;
    const lower = content.toLowerCase();
    // Псевдо-RRF-score: число вхождений терминов (приближение релевантности для превью).
    const score = terms.reduce((s, t) => s + lower.split(t).length - 1, 0);
    if (score === 0) continue;
    const idx = lower.indexOf(terms[0]);
    const snippet = content.slice(Math.max(0, idx - 40), idx + 200).replace(/\s+/g, ' ').trim();
    hits.push({ chunkId: hits.length, path, title: null, headingPath: null, snippet, score });
  }
  return hits.sort((a, b) => b.score - a.score || a.path.localeCompare(b.path)).slice(0, limit);
}

/** Предложения связей для превью/тестов: общие слова с другими (незалинкованными) заметками. */
export async function getLinkSuggestions(path: string, limit = 5): Promise<LinkSuggestion[]> {
  const raw = CONTENT[path];
  if (!raw) return [];
  const terms = new Set(
    raw
      .toLowerCase()
      .split(/[^\p{L}\p{N}]+/u)
      .filter((t) => t.length > 3),
  );
  const linked = new Set<string>();
  const re = /\[\[([^\]\n]+?)\]\]/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(raw)) !== null) {
    const tgt = m[1].split('|')[0].split('#')[0].trim();
    linked.add(tgt);
    linked.add(`${tgt}.md`);
  }

  const out: LinkSuggestion[] = [];
  for (const [p, c] of Object.entries(CONTENT)) {
    if (p === path || linked.has(p) || linked.has(p.replace(/\.md$/, ''))) continue;
    const words = c.toLowerCase().split(/[^\p{L}\p{N}]+/u);
    const overlap = words.filter((w) => terms.has(w)).length;
    if (overlap === 0) continue;
    out.push({
      path: p,
      title: null,
      score: overlap / Math.max(words.length, 1),
      reason: c.slice(0, 120).replace(/\s+/g, ' ').trim(),
    });
  }
  return out.sort((a, b) => b.score - a.score).slice(0, limit);
}

/** Мок «Похожих заметок» (#35): как getLinkSuggestions, но ВКЛЮЧАЯ уже связанные (дискавери). */
export async function getRelatedNotes(path: string, limit = 12): Promise<LinkSuggestion[]> {
  const raw = CONTENT[path];
  if (!raw) return [];
  const terms = new Set(
    raw
      .toLowerCase()
      .split(/[^\p{L}\p{N}]+/u)
      .filter((t) => t.length > 3),
  );
  const out: LinkSuggestion[] = [];
  for (const [p, c] of Object.entries(CONTENT)) {
    if (p === path) continue; // исключаем только сам файл; уже связанные — ВКЛЮЧАЕМ
    const words = c.toLowerCase().split(/[^\p{L}\p{N}]+/u);
    const overlap = words.filter((w) => terms.has(w)).length;
    if (overlap === 0) continue;
    out.push({
      path: p,
      title: null,
      score: overlap / Math.max(words.length, 1),
      reason: c.slice(0, 120).replace(/\s+/g, ' ').trim(),
    });
  }
  return out.sort((a, b) => b.score - a.score).slice(0, limit);
}

/** Мок «Целей» (#35) для превью/тестов: пара целей с прогрессом + одна без (D7). */
export async function getGoals(): Promise<GoalEntry[]> {
  return [
    { path: 'Цели/Книга.md', title: 'Дописать книгу', progress: 65 },
    { path: 'Цели/Марафон.md', title: 'Пробежать марафон', progress: 30 },
    { path: 'Цели/Идея.md', title: 'Идея без прогресса', progress: null },
  ];
}

/** Мок «Дайджеста изменений» (ADR-007 slice 4) для превью/тестов: один пример дайджеста. */
export async function getDigest(): Promise<Digest> {
  return {
    createdAt: 1_733_000_000,
    since: 1_732_913_600,
    noteCount: 3,
    content:
      '- Доработана глава про введение в книге.\n- Обновлён план тренировок к марафону.\n- Зафиксирована новая идея для проекта.',
  };
}

/** Симуляция RAG-чат-стрима для превью/тестов: sources → токены (по словам) → done. */
export function streamChat(
  question: string,
  onEvent: (event: ChatStreamEvent) => void,
  opts: { k?: number; grounded?: boolean; web?: boolean } = {},
): () => void {
  const { k = 8, grounded = true, web = false } = opts;
  let cancelled = false;
  void (async () => {
    let answer: string;
    if (web) {
      // W-2 (мок): web-агент «нашёл» источники в SearXNG и отвечает с цитатами.
      onEvent({
        type: 'webSources',
        sources: [
          { title: 'Документация по теме', url: 'https://example.com/docs', snippet: 'Краткий фрагмент из найденной страницы.' },
          { title: 'Обсуждение на форуме', url: 'https://forum.example.com/t/123', snippet: 'Ещё один релевантный результат поиска.' },
        ],
      });
      answer = `По результатам веб-поиска: ${question} — см. источники [1][2].`;
    } else if (grounded) {
      const sources = await searchContent(question, { limit: k });
      if (cancelled) return;
      onEvent({ type: 'sources', sources });
      answer = sources.length
        ? `На основе заметок: ${sources[0].snippet.slice(0, 80)}… [1]`
        : 'Не нашёл ответа в ваших заметках.';
    } else {
      // V4.4 общий чат: без ретрива, источники пустые.
      onEvent({ type: 'sources', sources: [] });
      answer = `(общий чат) Отвечаю напрямую: ${question}`;
    }
    // R1 (мок): живая сводка размышления стримится в индикатор «думает» до ответа (как gemma+Qwen
    // в реале — обновляется по ходу). Сырой CoT не эмитим: фронт его всё равно не рендерит.
    onEvent({ type: 'reasoningSummary', text: 'Анализирую вопрос' });
    await new Promise((r) => setTimeout(r, 15));
    if (cancelled) return;
    onEvent({ type: 'reasoningSummary', text: 'Формулирую ответ' });
    for (const tok of answer.split(/(\s+)/)) {
      if (cancelled) return;
      await new Promise((r) => setTimeout(r, 15));
      onEvent({ type: 'token', text: tok });
    }
    if (!cancelled) onEvent({ type: 'done', full: answer });
  })();
  return () => {
    cancelled = true;
  };
}

/** Мок «Противоречий» (#vision) для превью/тестов: пара примеров разных типов. */
export async function getContradictions(): Promise<Contradiction[]> {
  return [
    {
      pathA: 'Notes/Idea.md',
      pathB: 'Notes/Meeting.md',
      ctype: 'temporal',
      explanation: 'В «Idea» план на Q1, в «Meeting» он уже перенесён на Q2 — одна заметка устарела.',
      createdAt: 1_733_000_000,
    },
    {
      pathA: 'Projects/Roadmap.md',
      pathB: 'Projects/Alpha/Spec.md',
      ctype: 'hard',
      explanation: 'Roadmap обещает фичу X, спека Alpha явно выносит X из скоупа — прямое противоречие.',
      createdAt: 1_733_000_000,
    },
  ];
}

/** Симуляция inline-стрима (IL-2) для превью/тестов: несколько токенов по режиму → done. */
export function streamInline(
  mode: InlineMode,
  onEvent: (event: InlineStreamEvent) => void,
): () => void {
  let cancelled = false;
  const text =
    mode === 'summarize'
      ? 'Кратко: основная мысль фрагмента.'
      : mode === 'rewrite'
        ? 'Переписанный, более ясный вариант фрагмента.'
        : ' и продолжается естественно дальше.';
  void (async () => {
    for (const tok of text.split(/(\s+)/)) {
      if (cancelled) return;
      await new Promise((r) => setTimeout(r, 15));
      onEvent({ type: 'token', text: tok });
    }
    if (!cancelled) onEvent({ type: 'done', full: text });
  })();
  return () => {
    cancelled = true;
  };
}

/** Инлайн-теги файла (без `#`, отсортированы) — зеркало `file_tags` для графа. */
function tagsOf(path: string): string[] {
  const found = new Set<string>();
  const re = /(^|\s)#([\p{L}\p{N}_/-]+)/gu;
  let m: RegExpExecArray | null;
  while ((m = re.exec(CONTENT[path] ?? '')) !== null) found.add(m[2]);
  return [...found].sort();
}

/** Неориентированная смежность по `[[wikilink]]` во всём CONTENT (общая для local/full). */
function buildAdjacency(): Map<string, Set<string>> {
  const paths = Object.keys(CONTENT);
  const resolveTarget = (t: string): string | null => {
    const want = t.endsWith('.md') ? t.slice(0, -3) : t;
    return (
      paths.find(
        (p) =>
          p === t ||
          p.replace(/\.md$/, '') === want ||
          basename(p.replace(/\.md$/, '')) === basename(want),
      ) ?? null
    );
  };
  const adj = new Map<string, Set<string>>();
  const link = (a: string, b: string) => {
    (adj.get(a) ?? adj.set(a, new Set()).get(a)!).add(b);
    (adj.get(b) ?? adj.set(b, new Set()).get(b)!).add(a);
  };
  for (const [src, content] of Object.entries(CONTENT)) {
    const re = /\[\[([^\]\n]+?)\]\]/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(content)) !== null) {
      const tgt = resolveTarget(m[1].split('|')[0].split('#')[0].trim());
      if (tgt && tgt !== src) link(src, tgt);
    }
  }
  return adj;
}

/** Рёбра среди множества узлов `inSet` (дедуп неориентированных пар). */
function edgesAmong(
  inSet: Set<string>,
  adj: Map<string, Set<string>>,
  idOf: (p: string) => number,
): GraphEdge[] {
  const edges: GraphEdge[] = [];
  const seen = new Set<string>();
  for (const a of inSet)
    for (const b of adj.get(a) ?? [])
      if (inSet.has(b)) {
        const key = [a, b].sort().join('|');
        if (!seen.has(key)) {
          seen.add(key);
          edges.push({ source: idOf(a), target: idOf(b) });
        }
      }
  return edges;
}

export async function getLocalGraph(center: string, hops: number): Promise<GraphData> {
  if (!CONTENT[center]) return { nodes: [], edges: [] };
  const paths = Object.keys(CONTENT);
  const idOf = (p: string) => paths.indexOf(p);
  const adj = buildAdjacency();

  const inSet = new Set([center]);
  let frontier = [center];
  for (let h = 0; h < hops; h++) {
    const next: string[] = [];
    for (const f of frontier)
      for (const n of adj.get(f) ?? [])
        if (!inSet.has(n)) {
          inSet.add(n);
          next.push(n);
        }
    frontier = next;
  }

  const nodes = [...inSet].map((p) => ({ id: idOf(p), path: p, title: null, tags: tagsOf(p) }));
  return { nodes, edges: edgesAmong(inSet, adj, idOf) };
}

/** Единый граф всего vault — топ-`limit` файлов по степени связности + рёбра (AC-DOD-Ф3). */
export async function getFullGraph(limit: number): Promise<FullGraph> {
  const paths = Object.keys(CONTENT);
  const idOf = (p: string) => paths.indexOf(p);
  const adj = buildAdjacency();
  const byDegree = [...paths].sort(
    (a, b) => (adj.get(b)?.size ?? 0) - (adj.get(a)?.size ?? 0),
  );
  const chosen = byDegree.slice(0, Math.max(1, limit));
  const inSet = new Set(chosen);
  const nodes = chosen.map((p) => ({ id: idOf(p), path: p, title: null, tags: tagsOf(p) }));
  return {
    nodes,
    edges: edgesAmong(inSet, adj, idOf),
    totalFiles: paths.length,
    truncated: paths.length > chosen.length,
  };
}

export async function listNotes(query?: string, limit?: number): Promise<NoteRef[]> {
  const files = Object.values(TREE)
    .flat()
    .filter((e) => !e.isDir)
    .map((e) => ({ path: e.path, title: null }));
  // Уникализируем по пути.
  const seen = new Set<string>();
  let notes = files.filter((n) => (seen.has(n.path) ? false : (seen.add(n.path), true)));
  // #22: зеркало бэкенд-семантики — подстрочный фильтр (lowercase) + limit.
  const q = (query ?? '').trim().toLowerCase();
  if (q) notes = notes.filter((n) => n.path.toLowerCase().includes(q));
  return limit != null ? notes.slice(0, limit) : notes;
}

/** Зеркало бэкенд-резолва `[[ссылки]]` (#22): точный путь / +`.md` / basename ± `.md`. Без алиасов
 * (mock не индексирует frontmatter). */
export async function resolveNote(target: string): Promise<string | null> {
  const notes = await listNotes();
  const want = target.endsWith('.md') ? target.slice(0, -3) : target;
  const base = (p: string) => {
    const b = p.slice(p.lastIndexOf('/') + 1);
    return b.endsWith('.md') ? b.slice(0, -3) : b;
  };
  return (
    notes.find((n) => n.path === target)?.path ??
    notes.find((n) => n.path.replace(/\.md$/, '') === want)?.path ??
    notes.find((n) => base(n.path) === base(want))?.path ??
    null
  );
}
