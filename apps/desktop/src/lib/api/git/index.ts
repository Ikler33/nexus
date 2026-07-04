import * as mockGit from '../../mock/git';
import { bridge } from '../bridge';
import type {
  GitCommitOutcome,
  GitMergePreview,
  GitPullOutcome,
  GitResolution,
  GitStatusEntry,
} from './types';

/**
 * Git-домен (F-2d): git-sync vault (Ф3/Ф4) — статус рабочего дерева, коммит (полный/выборочный) с
 * secret-scan, токен доступа в keychain, remote-URL, синхронизация pull(ff)→push, превью и резолв
 * 3-way merge. Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/git`); потребители ходят сюда
 * по-прежнему через `tauriApi.git` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const git = {
  /** Статус рабочего дерева vault (изменённые/новые/удалённые, без игнорируемых). Ф3. */
  status: (): Promise<GitStatusEntry[]> =>
    bridge<GitStatusEntry[]>('git_status', undefined, () => mockGit.status()),

  /** Коммит изменений: secret-scan → при находке блокировка; пустое сообщение → авто-саммари. */
  commit: (message?: string): Promise<GitCommitOutcome> =>
    bridge<GitCommitOutcome>('git_commit', { message }, () => mockGit.commit(message)),

  /** Выборочный коммит (#10): коммитит ТОЛЬКО выбранные пути (из `git.status()`), а не всё-или-ничего.
   *  Secret-scan по выбранным; устаревший/пустой выбор → `nothing-to-commit`. Вне Tauri — мок. */
  commitPaths: (paths: string[], message?: string): Promise<GitCommitOutcome> =>
    bridge<GitCommitOutcome>('git_commit_paths', { paths, message }, () =>
      mockGit.commitPaths(paths, message),
    ),

  /** Сохранить токен доступа к remote в системном keychain (на диск не пишется). Ф3-3b. */
  setToken: (token: string): Promise<void> =>
    bridge<void>('git_set_token', { token }, () => mockGit.setToken(token)),

  /** Удалить токен из keychain. */
  clearToken: (): Promise<void> =>
    bridge<void>('git_clear_token', undefined, () => mockGit.clearToken()),

  /** Есть ли сохранённый токен (для UI «подключено»). */
  hasToken: (): Promise<boolean> =>
    bridge<boolean>('git_has_token', undefined, () => mockGit.hasToken()),

  /** Установить URL remote `origin`. */
  setRemote: (url: string): Promise<void> =>
    bridge<void>('git_set_remote', { url }, () => mockGit.setRemote(url)),

  /** URL remote `origin` (или null). */
  getRemote: (): Promise<string | null> =>
    bridge<string | null>('git_get_remote', undefined, () => mockGit.getRemote()),

  /** Синхронизация с remote: pull (ff) → push. Токен берётся из keychain. */
  sync: (): Promise<GitPullOutcome> =>
    bridge<GitPullOutcome>('git_sync', undefined, () => mockGit.sync()),

  /** Превью merge с origin (in-memory): up-to-date / clean / конфликты (3-way). Ф4-8. */
  mergePreview: (): Promise<GitMergePreview> =>
    bridge<GitMergePreview>('git_merge_preview', undefined, () => mockGit.mergePreview()),

  /** Применить разрешённый merge (resolutions: [path, content]) + push. Возвращает oid коммита. */
  resolveConflicts: (theirs: string, resolutions: GitResolution[]): Promise<string> =>
    bridge<string>('git_resolve_conflicts', { theirs, resolutions }, () =>
      mockGit.resolveConflicts(theirs, resolutions),
    ),
};
