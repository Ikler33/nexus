import type { GitCommitOutcome, GitStatusEntry } from '../tauri-api';

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

export async function commit(): Promise<GitCommitOutcome> {
  if (dirty.length === 0) return { status: 'nothing-to-commit' };
  const files = dirty.length;
  dirty = [];
  return { status: 'committed', oid: 'mock0a1b2c3', message: `Vault sync: ~${files} changed`, files };
}
