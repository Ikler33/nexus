import type { GitCommitOutcome, GitPullOutcome, GitStatusEntry } from '../tauri-api';

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
