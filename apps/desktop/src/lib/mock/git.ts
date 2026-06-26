import type {
  GitCommitOutcome,
  GitMergePreview,
  GitPullOutcome,
  GitResolution,
  GitStatusEntry,
} from '../tauri-api';

/**
 * Мок git-sync для превью/тестов: набор «изменённых» файлов, `commit` их «коммитит» (очищает).
 * Реальная логика (libgit2 + secret-scan + блокировка) — в Rust `src/git`. Здесь — happy-path для UI.
 */

let dirty: GitStatusEntry[] = [
  { path: 'README.md', kind: 'modified' },
  { path: 'Notes/Idea.md', kind: 'new' },
];

export async function status(): Promise<GitStatusEntry[]> {
  return [...dirty];
}

export async function commit(message?: string): Promise<GitCommitOutcome> {
  if (dirty.length === 0) return { status: 'nothing-to-commit' };
  const files = dirty.length;
  dirty = [];
  // Зеркалит бэкенд `commit_all_with_message`: пользовательское сообщение → авто-саммари при пустом.
  return {
    status: 'committed',
    oid: 'mock0a1b2c3',
    message: message?.trim() || `Vault sync: ~${files} changed`,
    files,
  };
}

/**
 * Выборочный коммит (#10) — зеркалит Rust `git_commit_paths`/`commit_paths_with_message`:
 * коммитит ТОЛЬКО пересечение `paths` с реальными изменениями (`dirty`), остальное остаётся
 * НЕ закоммиченным (видно в следующем `status()` — превью/тесты не врут, что всё ушло).
 * Пустое пересечение (устаревший/пустой выбор) → `nothing-to-commit` (как бэкенд). Secret-scan —
 * бэкенд-only, мок его не моделирует (happy-path).
 */
export async function commitPaths(paths: string[], message?: string): Promise<GitCommitOutcome> {
  const sel = dirty.filter((d) => paths.includes(d.path));
  if (sel.length === 0) return { status: 'nothing-to-commit' };
  dirty = dirty.filter((d) => !paths.includes(d.path)); // убираем ТОЛЬКО выбранные; остальное dirty
  return {
    status: 'committed',
    oid: 'mock0a1b2c3',
    message: message?.trim() || `Vault sync: ~${sel.length} changed`,
    files: sel.length,
  };
}

// Мок keychain-токена (в реальности — системный keychain через Rust `keyring`).
let token: string | null = null;
export async function setToken(t: string): Promise<void> {
  token = t;
}
export async function clearToken(): Promise<void> {
  token = null;
}
export async function hasToken(): Promise<boolean> {
  return token !== null;
}

let remote: string | null = null;
export async function setRemote(url: string): Promise<void> {
  remote = url;
}
export async function getRemote(): Promise<string | null> {
  return remote;
}
export async function sync(): Promise<GitPullOutcome> {
  // Мок: успешный fast-forward (реально — pull+push через git2 по токену из keychain).
  return { status: 'fast-forward', oid: 'mockff1234567' };
}

export async function mergePreview(): Promise<GitMergePreview> {
  // Мок: один конфликтный файл (для превью/разработки resolver-панели).
  return {
    status: 'conflicts',
    theirs: 'mocktheirs789abc',
    files: [
      {
        path: 'README.md',
        base: '# Mock Vault\n\nОбщая база до расхождения.\n',
        ours: '# Mock Vault\n\nНаша правка этой строки.\n',
        theirs: '# Mock Vault\n\nИх правка той же строки.\n',
      },
    ],
  };
}

export async function resolveConflicts(
  theirs: string,
  resolutions: GitResolution[],
): Promise<string> {
  dirty = [];
  // Мок: «слили» merge с theirs, применив N резолвов.
  return `mockmerge_${theirs.slice(0, 6)}_${resolutions.length}`;
}

/**
 * ТЕСТ-хелпер: сброс module-level стейта мока к сид-значениям (этот мок мутабелен — `commit`/
 * `commitPaths`/`resolveConflicts` меняют общий `dirty`; токен/remote тоже module-level). Зови в
 * `beforeEach`, чтобы тесты не связывались скрыто через общий мок (известный footgun кодбейза).
 */
export function __resetGitMock(): void {
  dirty = [
    { path: 'README.md', kind: 'modified' },
    { path: 'Notes/Idea.md', kind: 'new' },
  ];
  token = null;
  remote = null;
}
