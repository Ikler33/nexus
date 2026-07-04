import type {
  BacklinkEntry,
  MentionEntry,
  ChatStreamEvent,
  Contradiction,
  Digest,
  EpisodeHit,
  FileEntry,
  FullGraph,
  GoalEntry,
  GraphData,
  GraphEdge,
  InlineMode,
  InlineStreamEvent,
  LinkSuggestion,
  MemoryHit,
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
    dir('Templates', true),
    dir('Empty', false),
    file('README.md', 1200),
    file('Inbox.md', 340),
  ],
  Templates: [file('Templates/Meeting.md', 180), file('Templates/Daily.md', 90)],
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
  // Шаблоны (CAP-3): плейсхолдеры {{date}}/{{time}}/{{title}} подставляются при создании.
  'Templates/Meeting.md': '# {{title}}\n\nДата: {{date}} {{time}}\n\n## Повестка\n- \n\n## Решения\n- \n',
  'Templates/Daily.md': '# {{date}}\n\n## Задачи\n- \n\n## Мысли\n- \n',
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

/** BOARD-1: JS-порт бэкенд-`set_frontmatter_field` (мок зеркалит контракт — иначе превью/тесты «зелёные»
 *  на неверном поведении, урок MEM-5). Валидирует ключ (как `value_key`), правит/добавляет один плоский
 *  ключ (дубль → последнее вхождение), нет блока — создаёт; незакрытый `---` → throw (Malformed);
 *  значение без round-trip (перевод строки/краевые кавычки/инлайн-список) → throw (Unrepresentable);
 *  целевой ключ уже хранит СПИСОК/блок-родитель → throw (NonScalarTarget) — файл НЕ трогаем (m8). */
export async function setFrontmatterField(
  path: string,
  key: string,
  value: string,
): Promise<{ content: string; hash: string }> {
  const src = CONTENT[path] ?? '';
  const next = setFmField(src, valueKey(key), value);
  CONTENT[path] = next;
  return { content: next, hash: mockHash(next) };
}

/** Зеркало Rust `value_key`: имя свойства — идентификатор (буквы/цифры/`_`/`-`), триммится. */
function valueKey(key: string): string {
  const k = key.trim();
  if (k === '' || !/^[\p{L}\p{N}_-]+$/u.test(k)) throw new Error(`недопустимый ключ свойства: «${key}»`);
  return k;
}
function quoteYaml(v: string): boolean {
  if (v === '' || v !== v.trim()) return true;
  const f = v[0];
  if ('!&*?|>%@`"\'#,[]{}'.includes(f)) return true;
  if ((f === '-' || f === '?' || f === ':') && (v.length === 1 || v[1] === ' ')) return true;
  if (v.endsWith(':') || v.includes(' #') || v.includes(': ')) return true;
  return ['null', '~', 'true', 'false', 'yes', 'no', 'on', 'off'].includes(v.toLowerCase());
}
/** Зеркало Rust `read_scalar`: edge-stripper — краевые пробелы → краевые `"`/`'` → снова пробелы. */
function readScalar(raw: string): string {
  const s = raw.trim();
  let i = 0;
  let j = s.length;
  while (i < j && (s[i] === '"' || s[i] === "'")) i++;
  while (j > i && (s[j - 1] === '"' || s[j - 1] === "'")) j--;
  return s.slice(i, j).trim();
}
/** Зеркало Rust `fm_value_repr`: кодирует значение, ЕСЛИ читатель прочёл бы его обратно тем же; иначе null. */
function fmValueRepr(value: string): string | null {
  if (value === '' || value.includes('\n') || value.includes('\r')) return null;
  const quoted = quoteYaml(value) ? `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"` : value;
  const read = readScalar(quoted);
  if (read !== value || read.startsWith('[') || read.startsWith('{')) return null;
  return quoted;
}
function isFieldLine(line: string, key: string): boolean {
  if (/^[ \t-]/.test(line)) return false;
  const c = line.indexOf(':');
  if (c < 0) return false;
  const k = line.slice(0, c).trim();
  return k === key && /^[\p{L}\p{N}_-]+$/u.test(k);
}
/** Зеркало Rust `is_block_scalar_indicator`: голый YAML `|`/`>` с опц. chomp/indent (`|`,`>`,`|-`,`>2`). */
function isBlockScalarIndicator(value: string): boolean {
  return /^[|>][+\-0-9]*$/.test(value);
}
/** m8: зеркало Rust `is_non_scalar_target` — текущее значение совпавшего ключа НЕ плоский скаляр
 *  (инлайн-список/объект `[`/`{`; либо пустое/`|`/`>` + ниже отступной дочерний блок или `- …`).
 *  Блок через пустую строку НЕ ловим — симметрично читателю. Строки без EOL. */
function isNonScalarTarget(keyLine: string, nextLine: string | undefined): boolean {
  const c = keyLine.indexOf(':');
  if (c < 0) return false;
  const value = readScalar(keyLine.slice(c + 1));
  if (value.startsWith('[') || value.startsWith('{')) return true;
  if ((value === '' || isBlockScalarIndicator(value)) && nextLine !== undefined) {
    const trimmed = nextLine.trimStart();
    return nextLine.length !== trimmed.length || trimmed.startsWith('-');
  }
  return false;
}
function setFmField(content: string, key: string, value: string): string {
  const quoted = fmValueRepr(value);
  if (quoted === null) throw new Error('Unrepresentable frontmatter value (перевод строки/краевые кавычки)');
  if (!content.startsWith('---\n') && !content.startsWith('---\r\n')) {
    return `---\n${key}: ${quoted}\n---\n\n${content}`;
  }
  const lines = content.split('\n');
  let close = -1;
  for (let i = 1; i < lines.length; i++) {
    if (lines[i].replace(/\r$/, '') === '---') {
      close = i;
      break;
    }
  }
  if (close < 0) throw new Error('Malformed frontmatter (незакрытый ---)');
  // Правим ПОСЛЕДНЕЕ совпадение (читатель: last-key-wins).
  let last = -1;
  for (let i = 1; i < close; i++) {
    if (isFieldLine(lines[i].replace(/\r$/, ''), key)) last = i;
  }
  if (last >= 0) {
    const bare = lines[last].replace(/\r$/, '');
    const nextBare = last + 1 < close ? lines[last + 1].replace(/\r$/, '') : undefined;
    // m8: ключ хранит список/блок-родитель — перезапись скаляром осиротила бы `- a`/`- b` или
    // потеряла бы инлайн-список → throw (NonScalarTarget), CONTENT НЕ мутируем (файл цел).
    if (isNonScalarTarget(bare, nextBare)) {
      throw new Error('NonScalarTarget: свойство хранит список — нельзя перезаписать одним значением');
    }
    const cr = lines[last].endsWith('\r') ? '\r' : '';
    lines[last] = `${bare.slice(0, bare.indexOf(':'))}: ${quoted}${cr}`;
  } else {
    // EOL новой строки — как у блока (CRLF, если открывающий `---` был CRLF).
    const cr = content.startsWith('---\r\n') ? '\r' : '';
    lines.splice(close, 0, `${key}: ${quoted}${cr}`);
  }
  return lines.join('\n');
}

export async function deletePath(path: string): Promise<void> {
  // Убираем элемент из родительского каталога дерева.
  const parent = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
  if (TREE[parent]) TREE[parent] = TREE[parent].filter((e) => e.path !== path);
  // Сносим контент и поддеревья (для каталога).
  const under = (p: string) => p === path || p.startsWith(`${path}/`);
  for (const key of Object.keys(TREE)) if (under(key)) delete TREE[key];
  for (const key of Object.keys(CONTENT)) if (under(key)) delete CONTENT[key];
}

export async function renamePath(from: string, to: string): Promise<void> {
  const swap = (p: string) =>
    p === from ? to : p.startsWith(`${from}/`) ? `${to}${p.slice(from.length)}` : p;
  // Переносим контент под новый путь (включая поддерево).
  for (const key of Object.keys(CONTENT)) {
    const np = swap(key);
    if (np !== key) {
      CONTENT[np] = CONTENT[key];
      delete CONTENT[key];
    }
  }
  // Чиним дерево: убираем элемент из старого родителя, добавляем в новый (упрощённо — для UI-тестов).
  const fromParent = from.includes('/') ? from.slice(0, from.lastIndexOf('/')) : '';
  const toParent = to.includes('/') ? to.slice(0, to.lastIndexOf('/')) : '';
  const moved = (TREE[fromParent] ?? []).find((e) => e.path === from);
  if (TREE[fromParent]) TREE[fromParent] = TREE[fromParent].filter((e) => e.path !== from);
  if (moved) {
    const name = to.slice(to.lastIndexOf('/') + 1);
    (TREE[toParent] ??= []).push({ ...moved, path: to, name });
  }
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

export async function getUnlinkedMentions(path: string): Promise<MentionEntry[]> {
  const noExt = path.endsWith('.md') ? path.slice(0, -3) : path;
  // Ключи как на бэке: имя файла (basename) + H1-заголовок заметки. Гард короткого ключа (шум).
  const stem = basename(noExt);
  const h1 = (CONTENT[path]?.match(/^#\s+(.+)$/m)?.[1] ?? '').trim();
  const keys = [stem, h1].filter((k) => k.length >= 3).map((k) => k.toLowerCase());
  if (keys.length === 0) return [];
  const linkers = new Set((await getBacklinks(path)).map((b) => b.sourcePath));
  const out: MentionEntry[] = [];
  for (const [src, content] of Object.entries(CONTENT)) {
    if (src === path || linkers.has(src)) continue;
    const lc = content.toLowerCase();
    const hit = keys.find((k) => lc.includes(k));
    if (hit === undefined) continue;
    const idx = lc.indexOf(hit);
    const start = Math.max(0, idx - 20);
    const end = Math.min(content.length, idx + hit.length + 30);
    out.push({
      sourcePath: src,
      sourceTitle: null,
      snippet: `…${content.slice(start, end).replace(/\n/g, ' ').trim()}…`,
    });
  }
  return out.slice(0, 30);
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

  // P0-2: граф-ранг от «центра» (открытого файла) — наблюдаемое зеркало `SearchOptions.center`
  // бэкенд-hybrid_search: соседи центра по графу вики-ссылок ранжируются выше (буст к score).
  const centerNeighbors = opts?.center ? (buildAdjacency().get(opts.center) ?? null) : null;

  const hits: SearchHit[] = [];
  for (const [path, content] of Object.entries(CONTENT)) {
    if (opts?.folder && path !== opts.folder && !path.startsWith(`${opts.folder}/`)) continue;
    const lower = content.toLowerCase();
    // Псевдо-RRF-score: число вхождений терминов (приближение релевантности для превью) + граф-буст.
    let score = terms.reduce((s, t) => s + lower.split(t).length - 1, 0);
    if (score === 0) continue;
    if (centerNeighbors?.has(path)) score += 2;
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

/** Зеркало `generate_digest`: в браузере воркера-планировщика нет — no-op (событие `jobs:changed`
 *  мок-бэкенд не эмитит). Инлайн-заглушка переехала из tauri-api.ts (ratchet parity-гейта (в), F-2d). */
export async function generateDigest(): Promise<void> {}

// ── P0-2: мок-чат — ПОЛНОЕ зеркало входа/событий команды `chat_rag` ──────────────────────────────
// Константы = бэкенд-константам (chat.rs / search::rerank / episode) — чтобы капы были наблюдаемо
// теми же: DEFAULT_K=8, MEMORY_K=3, EPISODE_K=2, RERANK_RETRIEVE=24, PINNED_MAX_NOTES=5.
const DEFAULT_K = 8;
const MEMORY_K = 3;
const EPISODE_K = 2;
const RERANK_RETRIEVE = 24;
const PINNED_MAX_NOTES = 5;

/** Память переписки (N4b, зеркало Rust `chat_log::MemoryHit` — camelCase на проводе): фейковые
 *  фрагменты прошлых диалогов для чипов «из прошлых разговоров» в превью/тестах. */
const MOCK_MEMORY: MemoryHit[] = [
  { sessionId: 101, sessionTitle: 'Планирование Alpha', role: 'user', snippet: 'Договорились: спека Alpha — приоритет недели.', score: 0.62 },
  { sessionId: 102, sessionTitle: 'Разбор Inbox', role: 'assistant', snippet: 'Предлагал переносить быстрые заметки из Inbox в проекты.', score: 0.54 },
  { sessionId: 103, sessionTitle: 'Идеи по Roadmap', role: 'user', snippet: 'Хотели добавить в Roadmap веху про поиск.', score: 0.47 },
  { sessionId: 104, sessionTitle: 'Ретро недели', role: 'assistant', snippet: 'Итог ретро: меньше контекст-свитчинга.', score: 0.41 },
];

/** Эпизодическая память (EP-2, зеркало Rust `episode::EpisodeHit`): саммари прошлых сессий.
 *  Сессии 101/102 пересекаются с MOCK_MEMORY — чтобы дедуп «эпизод глушит сырые реплики» был
 *  наблюдаем (как exclude_sessions в `search_memory`). */
const MOCK_EPISODES: EpisodeHit[] = [
  { episodeId: 11, sessionId: 101, sessionTitle: 'Планирование Alpha', summarySnippet: 'Обсуждали приоритеты проекта Alpha: спека, ревью, дедлайны.', startedAt: 1_733_000_000, endedAt: 1_733_003_600, score: 0.58 },
  { episodeId: 13, sessionId: 102, sessionTitle: 'Разбор Inbox', summarySnippet: 'Разобрали входящие: быстрые заметки уехали в проекты.', startedAt: 1_732_950_000, endedAt: 1_732_953_600, score: 0.5 },
  { episodeId: 12, sessionId: 105, sessionTitle: 'Наброски статьи', summarySnippet: 'Собирали тезисы статьи о «втором мозге» и цитаты.', startedAt: 1_732_900_000, endedAt: 1_732_903_600, score: 0.44 },
];

/** Зеркало бэкенд-`is_pinnable` (chat.rs): в закреплённый контекст идут только `.md`-заметки БЕЗ
 *  dot-компонентов (`.nexus`/`.git`/dot-файлы) — анти-эксфильтрация секретов в LLM-канал. */
function isPinnable(path: string): boolean {
  return /\.md$/i.test(path) && !path.split('/').some((s) => s.startsWith('.'));
}

/** Мок LLM-реранка (зеркало пайплайна `search::rerank::llm_rerank`): ДРУГАЯ функция релевантности —
 *  число РАЗЛИЧНЫХ терминов запроса в пути+сниппете («понимание темы»), не сырая частота вхождений.
 *  Детерминирован; наблюдаемо меняет порядок против базового ретрива. */
function rerankHits(question: string, hits: SearchHit[]): SearchHit[] {
  const terms = question.trim().toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const distinct = (h: SearchHit) => {
    const hay = `${h.path} ${h.snippet}`.toLowerCase();
    return terms.filter((t) => hay.includes(t)).length;
  };
  return [...hits].sort(
    (a, b) => distinct(b) - distinct(a) || b.score - a.score || a.path.localeCompare(b.path),
  );
}

/** Опции мок-чата — зеркало ВХОДА `chat_rag` (все параметры фронта; «молча выброшенных» нет — P0-2).
 *  Дефолты = дефолтам бэкенда (`unwrap_or`): grounded=true, web=false, rerank=true, memory=true,
 *  agentMemory=false, episodic=false, deep=false, k=DEFAULT_K. */
export interface MockChatOpts {
  k?: number;
  center?: string;
  grounded?: boolean;
  web?: boolean;
  rerank?: boolean;
  memory?: boolean;
  agentMemory?: boolean;
  episodic?: boolean;
  deep?: boolean;
  sessionId?: number | null;
  pinned?: string[];
}

/**
 * Симуляция RAG-чат-стрима — зеркало ПОРЯДКА и СОБЫТИЙ `chat_rag` (Rust `ChatStreamEvent`):
 * (web → `webSources` | grounded → `sources` | общий → `sources:[]`) → `episodeSources`? →
 * `memorySources`? → (deep: `reasoning`… + живая `reasoningSummary`) → `token`… →
 * (deep: ФИНАЛЬНАЯ `reasoningSummary` — после конца токенов, как chat.rs) → `done` | `error`.
 *
 * P0-2 (mock-must-match-backend): ВСЕ опции команды приняты и наблюдаемы —
 * - `k` клампится 1..20 (зеркало `.clamp(1, 20)`);
 * - `center` — граф-буст соседей открытого файла (см. searchContent);
 * - `rerank` — ретрив глубже (RERANK_RETRIEVE) → LLM-переупорядочивание → обрезка до k;
 * - `memory`/`episodic` — чипы памяти: эпизоды считаются ПЕРВЫМИ, память исключает сессии,
 *   уже всплывшие эпизодом (дедуп, как exclude_sessions), и текущую `sessionId`;
 * - `agentMemory`/`pinned` — событий на проводе НЕТ (бэкенд молча подмешивает их в промпт) —
 *   наблюдаемость через текст ответа; pinned фильтруется зеркалом `is_pinnable` + бюджет 5;
 * - `deep` — reasoning-события и доп. задержка ТОЛЬКО в «Глубоком» (зеркало выбора chat vs
 *   chat_fast: «Быстрый» БЕЗ CoT → без 💭); живые сводки ПО ХОДУ + финальная ПОСЛЕ токенов;
 * - УЗКИЙ демо-маркер «демо-ошибка»/«demo-error» → терминальный `error` (с `deniedKind:'offline'`,
 *   если рядом «офлайн» — форма отказа эгресса AC-EGR-14); обычное слово «ошибка» мок не роняет.
 */
export function streamChat(
  question: string,
  onEvent: (event: ChatStreamEvent) => void,
  opts: MockChatOpts = {},
): () => void {
  const {
    k = DEFAULT_K,
    center,
    grounded = true,
    web = false,
    rerank = true,
    memory = true,
    agentMemory = false,
    episodic = false,
    deep = false,
    sessionId = null,
    pinned,
  } = opts;
  let cancelled = false;
  void (async () => {
    const kEff = Math.min(20, Math.max(1, Math.trunc(k))); // зеркало k.clamp(1, 20)
    let answer: string;
    if (web) {
      // W-2 (мок): web-агент «нашёл» источники в SearXNG и отвечает с цитатами. Успешный web-план
      // ЗАМЕЩАЕТ RAG-ветку (как в chat_rag) — событие `sources` не эмитится.
      onEvent({
        type: 'webSources',
        sources: [
          { title: 'Документация по теме', url: 'https://example.com/docs', snippet: 'Краткий фрагмент из найденной страницы.' },
          { title: 'Обсуждение на форуме', url: 'https://forum.example.com/t/123', snippet: 'Ещё один релевантный результат поиска.' },
        ],
      });
      answer = `По результатам веб-поиска: ${question} — см. источники [1][2].`;
    } else if (grounded) {
      // Реранк-пайплайн (зеркало chat_rag): do_rerank → ретрив глубже (RERANK_RETRIEVE), мок-«LLM»
      // переупорядочивает, обрезаем до k. Без реранка — прямой ретрив с limit=k.
      const retrieved = await searchContent(question, {
        limit: rerank ? RERANK_RETRIEVE : kEff,
        center,
      });
      if (cancelled) return;
      const sources = rerank ? rerankHits(question, retrieved).slice(0, kEff) : retrieved;
      onEvent({ type: 'sources', sources });
      answer = sources.length
        ? `На основе заметок: ${sources[0].snippet.slice(0, 80)}… [1]`
        : 'Не нашёл ответа в ваших заметках.';
    } else {
      // V4.4 общий чат: без ретрива, источники пустые.
      onEvent({ type: 'sources', sources: [] });
      answer = `(общий чат) Отвечаю напрямую: ${question}`;
    }

    // EP-2: эпизоды считаются ПЕРВЫМИ (для дедупа с памятью переписки), текущая сессия исключена,
    // кап EPISODE_K. Событие уходит только при непустых hits (зеркало `Ok(hits) if !hits.is_empty()`).
    const episodeSessions = new Set<number>();
    if (episodic) {
      const hits = MOCK_EPISODES.filter((e) => e.sessionId !== sessionId)
        .sort((a, b) => b.score - a.score)
        .slice(0, EPISODE_K);
      if (hits.length) {
        for (const h of hits) episodeSessions.add(h.sessionId);
        onEvent({ type: 'episodeSources', sources: hits });
      }
    }
    // N4b: память переписки — исключая текущую сессию И сессии, уже всплывшие эпизодом (дедуп
    // exclude_sessions), кап MEMORY_K. Только непустые.
    if (memory) {
      const hits = MOCK_MEMORY.filter(
        (m) => m.sessionId !== sessionId && !episodeSessions.has(m.sessionId),
      ).slice(0, MEMORY_K);
      if (hits.length) onEvent({ type: 'memorySources', sources: hits });
    }
    // MEM (D2): факты агента событий на проводе НЕ имеют (бэкенд подмешивает их в промпт) —
    // наблюдаемость мока через текст ответа (опция не выброшена молча).
    if (agentMemory) answer += ' (учтены факты памяти агента)';
    // P6-PIN: закреплённые — зеркало is_pinnable (.md без dot-компонентов) + бюджет PINNED_MAX_NOTES;
    // события нет (гарантированный контекст промпта) — наблюдаемость через ответ.
    const pinnedOk = (pinned ?? []).filter(isPinnable).slice(0, PINNED_MAX_NOTES);
    if (pinnedOk.length) answer += ` (закреплённых заметок в контексте: ${pinnedOk.length})`;

    // Триггер терминальной ошибки (провайдер упал / отказ эгресса): форма — зеркало Rust
    // `Error{message, denied_kind?}` → `{message, deniedKind?}` (AC-EGR-14 для i18n-баннера).
    // Маркер УЗКИЙ («демо-ошибка»/«demo-error») — анти-футган: легитимный вопрос «найди заметку
    // про ошибку» мок НЕ роняет (Playwright-смоук ходит по реальным фразам).
    if (/демо-ошибка|demo-error/i.test(question)) {
      onEvent(
        /офлайн|offline/i.test(question)
          ? { type: 'error', message: 'мок: эгресс запрещён (офлайн-режим)', deniedKind: 'offline' }
          : { type: 'error', message: 'мок: chat-провайдер недоступен' },
      );
      return;
    }

    // R1: reasoning-события — ТОЛЬКО «Глубокий» (deep → модель С CoT; «Быстрый» = chat_fast БЕЗ
    // reasoning → бэкенд не шлёт ни `reasoning`, ни `reasoningSummary`). Сырые дельты CoT (спойлер
    // «развернуть») + ЖИВАЯ сводка по ходу; deep заметно медленнее — доп. задержка.
    if (deep) {
      for (const delta of ['Смотрю найденные заметки. ', 'Сопоставляю факты и формулирую вывод.']) {
        if (cancelled) return;
        await new Promise((r) => setTimeout(r, 15));
        onEvent({ type: 'reasoning', text: delta });
      }
      onEvent({ type: 'reasoningSummary', text: 'Сопоставляю заметки и формулирую ответ' });
      await new Promise((r) => setTimeout(r, 30));
    }
    for (const tok of answer.split(/(\s+)/)) {
      if (cancelled) return;
      await new Promise((r) => setTimeout(r, 15));
      onEvent({ type: 'token', text: tok });
    }
    if (cancelled) return;
    // R1 (зеркало chat.rs: финал стрима): ФИНАЛЬНАЯ сводка по ПОЛНОМУ размышлению эмитится ПОСЛЕ
    // конца токенов, ПЕРЕД Done (короткий CoT мог не успеть тикнуть в живом таске-суммаризаторе) —
    // «summary всегда до token» на живом проводе НЕВЕРНО, мок это кодифицирует.
    if (deep) onEvent({ type: 'reasoningSummary', text: 'Свёл ответ по найденным заметкам' });
    onEvent({ type: 'done', full: answer });
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

// Тоггл «Поиск противоречий» (зеркало бэкенд-сеттинга `contradictions.enabled`, дефолт OFF). setEnabled
// персистит флаг в памяти процесса (мок-бэкенд без БД); kick-джобу не эмулируем.
let contradictionsEnabled = false;

export function contradictionsGetEnabled(): Promise<boolean> {
  return Promise.resolve(contradictionsEnabled);
}

export function contradictionsSetEnabled(on: boolean): Promise<void> {
  contradictionsEnabled = on;
  return Promise.resolve();
}

/** Зеркало `generate_contradictions`: в браузере воркера нет — no-op (событие `jobs:changed` не
 *  эмитится). Инлайн-заглушка переехала из tauri-api.ts (ratchet parity-гейта (в), F-2d). */
export async function generateContradictions(): Promise<void> {}

/** Мок краткого резюме заметки (Inspector «Резюме») — зеркалит контракт `get_note_summary`:
 *  пустой текст → null, иначе короткая сводка. Небольшая задержка имитирует LLM. */
export function noteSummary(text: string): Promise<string | null> {
  if (!text.trim()) return Promise.resolve(null);
  const words = text.trim().split(/\s+/).length;
  return new Promise((r) =>
    setTimeout(
      () => r(`Краткое содержание заметки (≈${words} слов): основная мысль и ключевые разделы.`),
      400,
    ),
  );
}

// ── F-2d: инлайн-заглушки suggest-домена переехали из tauri-api.ts (ratchet parity-гейта (в)) ────

/** Зеркало `explain_relation` (AIP-10): в браузере утилитарной LLM нет — '' (естественный фолбэк на
 *  сырой сниппет; тот же контракт, что у настоящей команды при отсутствии модели). */
export async function explainRelation(): Promise<string> {
  return '';
}

/** Зеркало `get_starting_questions` (AIP-SQ): в браузере утилитарной LLM нет — [] (фронт покажет
 *  статические подсказки; контракт настоящей команды при отсутствии модели/контента). */
export async function startingQuestions(): Promise<string[]> {
  return [];
}

/** Симуляция inline-стрима (IL-2) для превью/тестов: несколько токенов по режиму → done. */
export function streamInline(
  mode: InlineMode,
  onEvent: (event: InlineStreamEvent) => void,
  prompt?: string,
): () => void {
  let cancelled = false;
  const text =
    mode === 'summarize'
      ? 'Кратко: основная мысль фрагмента.'
      : mode === 'rewrite'
        ? 'Переписанный, более ясный вариант фрагмента.'
        : mode === 'prompt'
          ? // Зеркалит бэкенд: свободный запрос → сгенерированный текст для вставки (упоминает запрос).
            `Ответ на запрос «${(prompt ?? '').trim() || 'без запроса'}» на основе ваших заметок.`
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

const IMG_RE = /\.(png|jpe?g|gif|webp|svg|bmp|avif|ico)$/i;

/** Все файлы (не-папки) из TREE — для резолва вложений по basename (мок не индексирует картинки). */
function allFilePaths(): string[] {
  return Object.values(TREE)
    .flat()
    .filter((e) => !e.isDir)
    .map((e) => e.path);
}

/** Зеркало бэкенд-`watcher::is_ignored` (служебные пути): компонент `.nexus`/`.git`, dot-basename,
 *  `.conflict`. Ревью IMG-EMBED — мок обязан отвергать `.nexus/x.png` так же, как бэкенд (анти-утечка). */
function isIgnoredPath(p: string): boolean {
  const segs = p.split('/');
  if (segs.some((s) => s === '.nexus' || s === '.git')) return true;
  const base = segs[segs.length - 1] ?? '';
  return base.startsWith('.') || base.endsWith('.conflict');
}

/** Зеркало бэкенд-`resolve_attachment` (IMG-EMBED): путь-с-сепаратором → как есть (если существует и не
 *  служебный); голый basename → поиск картинки в TREE (кратчайший путь, регистронезависимо, мимо
 *  служебных). null — не найдено / служебное. */
export async function resolveAttachment(name: string): Promise<string | null> {
  if (!IMG_RE.test(name)) return null;
  if (name.includes('/') || name.includes('\\')) {
    const norm = name.replace(/\\/g, '/');
    if (isIgnoredPath(norm)) return null;
    return allFilePaths().includes(norm) ? norm : null;
  }
  const lower = name.toLowerCase();
  const hits = allFilePaths()
    .filter((p) => !isIgnoredPath(p) && p.slice(p.lastIndexOf('/') + 1).toLowerCase() === lower)
    .sort((a, b) => a.length - b.length);
  return hits[0] ?? null;
}

/** Зеркало бэкенд-`read_attachment` (мок): видимый placeholder-SVG для известной картинки (превью без
 *  Tauri показывает «картинку» с именем), иначе пусто (служебное/недоступное вложение). */
export async function readAttachment(path: string): Promise<string> {
  if (!IMG_RE.test(path) || isIgnoredPath(path) || !allFilePaths().includes(path)) return '';
  const label = path.slice(path.lastIndexOf('/') + 1);
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="220" height="140">` +
    `<rect width="220" height="140" rx="8" fill="#3b82f6" opacity="0.18"/>` +
    `<text x="110" y="74" font-family="sans-serif" font-size="13" fill="#3b82f6" ` +
    `text-anchor="middle">${label}</text></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

// ── F-2a: инлайн-заглушки vault-домена переехали из tauri-api.ts (ratchet parity-гейта (в)) ──────

/** Зеркало `write_attachment` (IMG-1): мок ФС не пишет — возвращает относительный путь `![](…)`,
 *  как настоящая команда. */
export async function writeAttachment(name: string): Promise<string> {
  return `attachments/${name}`;
}

/** Зеркало `rescan_vault`: в браузере индексатора нет — no-op (завершение не эмитится, как и
 *  остальные события мок-бэкенда). */
export async function rescan(): Promise<void> {}

/** Зеркало `notes_count` — статусбар «Проиндексировано · N» (DP-14): 847, как в демо-данных Home
 *  (`lib/mock/home.ts`). */
export async function notesCount(): Promise<number> {
  return 847;
}

/** Зеркало `file_mtime` — clock-чип doc-meta превью (DP-15): «3 ч назад». */
export async function fileMtime(): Promise<number> {
  return Math.floor(Date.now() / 1000) - 3 * 3600;
}
