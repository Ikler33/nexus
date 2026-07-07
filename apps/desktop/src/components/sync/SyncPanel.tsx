import { useCallback, useEffect, useState } from 'react';
import { GitBranch, GitMerge, RefreshCw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import {
  type GitCommitOutcome,
  type GitPullOutcome,
  type GitStatusEntry,
  tauriApi,
} from '../../lib/tauri-api';
import { useSyncStore } from '../../stores/sync';
import { useUIStore } from '../../stores/ui';
// ConflictResolver вынесен в `components/common` (F-10c) — общий safe-flow-компонент, чтобы App.tsx
// (statusbar-пилюля) тянул его вне вырезанной sync-зоны. Импорт из common легален и для sync-зоны.
import { ConflictResolver } from '../common/ConflictResolver';
import styles from './SyncPanel.module.css';

type SyncResult = GitPullOutcome | { status: 'error'; message: string };
// audit B13: коммит мог упасть на бэке (git/secret-scan), а UI глотал ошибку — добавляем error-исход.
type CommitOutcome = GitCommitOutcome | { status: 'error'; message: string };

/**
 * Панель синхронизации (Ф3, git-sync): изменения рабочего дерева + коммит (secret-scan на бэке),
 * настройка remote (URL + токен в системный keychain) и sync (pull-ff → push). Конфликт
 * (`merge-required`) разрешается в `ConflictResolver` (DP-10) — кнопкой отсюда или из
 * конфликт-пилюли статусбара (DP-14).
 */
export function SyncPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeSync);
  const [changes, setChanges] = useState<GitStatusEntry[] | null>(null);
  // P1-5: выбранные для коммита пути. После загрузки статуса дефолт = ВСЕ выбраны (дефолтный
  // «Коммит» = коммит всего, не регресс). До загрузки — пусто (кнопка и так gated по !changes).
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [outcome, setOutcome] = useState<CommitOutcome | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');

  // Перезагрузка статуса (load/после коммита) → реинициализируем выбор: все пути новых changes.
  // Невыбранные после выборочного коммита остаются в `changes` (мок/бэк их не закоммитили).
  const applyChanges = useCallback((entries: GitStatusEntry[]) => {
    setChanges(entries);
    setSelected(new Set(entries.map((e) => e.path)));
  }, []);

  // remote-конфиг
  const [remoteUrl, setRemoteUrl] = useState('');
  const [tokenInput, setTokenInput] = useState('');
  const [connected, setConnected] = useState(false);
  const [remoteError, setRemoteError] = useState<string | null>(null); // B16: сбой setRemote/setToken не глотаем
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null);
  const [syncBusy, setSyncBusy] = useState(false);
  const [resolving, setResolving] = useState(false);

  const load = useCallback(() => {
    setOutcome(null);
    setChanges(null);
    setSelected(new Set());
    tauriApi.git
      .status()
      .then(applyChanges)
      .catch(() => applyChanges([]));
    tauriApi.git
      .getRemote()
      .then((r) => setRemoteUrl(r ?? ''))
      .catch(() => {});
    tauriApi.git
      .hasToken()
      .then(setConnected)
      .catch(() => {});
  }, [applyChanges]);
  useEffect(() => load(), [load]);

  const commit = async () => {
    if (selected.size === 0 || !changes) return; // нечего коммитить (кнопка и так disabled)
    setBusy(true);
    try {
      const msg = message.trim() || undefined;
      // P1-5: всё выбрано → commit-all (не регресс); подмножество → commitPaths (только выбранные).
      const o =
        selected.size === changes.length
          ? await tauriApi.git.commit(msg)
          : await tauriApi.git.commitPaths([...selected], msg);
      if (o.status === 'committed') setMessage('');
      setOutcome(o);
      if (o.status !== 'blocked-by-secrets') {
        // Перезагрузка статуса: невыбранные при выборочном коммите ОСТАЮТСЯ dirty (мок зеркалит бэк).
        await tauriApi.git
          .status()
          .then(applyChanges)
          .catch(() => applyChanges([]));
      }
    } catch (e) {
      setOutcome({ status: 'error', message: String(e) }); // показываем сбой, а не глотаем (audit B13)
    } finally {
      setBusy(false);
    }
  };

  // Тоггл одного пути в выборе.
  const toggle = (path: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  // Мастер-чекбокс «выбрать все/ничего» в шапке секции «Изменения».
  const allSelected = !!changes && changes.length > 0 && selected.size === changes.length;
  const toggleAll = () => {
    setSelected(allSelected ? new Set() : new Set(changes?.map((c) => c.path) ?? []));
  };

  const saveRemote = async () => {
    setRemoteError(null);
    try {
      if (remoteUrl.trim()) await tauriApi.git.setRemote(remoteUrl.trim());
      if (tokenInput.trim()) {
        await tauriApi.git.setToken(tokenInput.trim());
        setTokenInput('');
      }
      setConnected(await tauriApi.git.hasToken());
    } catch (e) {
      setRemoteError(String(e)); // не молчим: пользователь думал, что подключил remote/токен (B16)
    }
  };

  // P1-17: отзыв сохранённого git-токена из keychain. Видна только когда `connected` (токен есть).
  // Реальный API `git.clearToken` (бэкенд `git_clear_token`); ошибку показываем как connect-флоу (B16).
  const clearToken = async () => {
    setRemoteError(null);
    try {
      await tauriApi.git.clearToken();
      setConnected(false);
      setTokenInput('');
    } catch (e) {
      setRemoteError(String(e)); // не глотаем: пользователь думал, что токен удалён (B16)
    }
  };

  const sync = async () => {
    setSyncBusy(true);
    setSyncResult(null);
    try {
      const result = await tauriApi.git.sync();
      setSyncResult(result);
      // DP-14: конфликт-пилюля статусбара живёт, пока merge не разрешён (и после закрытия панели).
      useSyncStore.getState().setMergeRequired(result.status === 'merge-required');
      await tauriApi.git
        .status()
        .then(applyChanges)
        .catch(() => {});
    } catch (e) {
      setSyncResult({ status: 'error', message: String(e) });
    } finally {
      setSyncBusy(false);
    }
  };

  return (
    <>
    <div className={styles.backdrop} onClick={close} role="presentation">
      <div
        className={styles.dialog}
        role="dialog"
        aria-modal="true"
        aria-label={t('git.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <span className={styles.headIco} aria-hidden>
            <GitBranch size={18} />
          </span>
          <span className={styles.title}>{t('git.title')}</span>
          <button
            className={styles.close}
            onClick={close}
            aria-label={t('git.close')}
            title={t('git.close')}
          >
            <X size={16} aria-hidden />
          </button>
        </header>

        <div className={styles.body}>
          {changes === null ? (
            <p className={styles.muted}>{t('git.loading')}</p>
          ) : changes.length === 0 ? (
            <p className={styles.muted}>{t('git.clean')}</p>
          ) : (
            <div>
              <div className={styles.secLabel}>
                <input
                  type="checkbox"
                  className={styles.selAll}
                  checked={allSelected}
                  onChange={toggleAll}
                  aria-label={t('git.selectAll')}
                />
                {t('git.changes')}
                <span className={styles.cnt}>{changes.length}</span>
              </div>
              <ul className={styles.changes} aria-label={t('git.changes')}>
                {changes.map((c) => {
                  const slash = c.path.lastIndexOf('/');
                  const dir = slash >= 0 ? c.path.slice(0, slash + 1) : '';
                  const name = slash >= 0 ? c.path.slice(slash + 1) : c.path;
                  return (
                    <li key={c.path} className={styles.change}>
                      <input
                        type="checkbox"
                        className={styles.check}
                        checked={selected.has(c.path)}
                        onChange={() => toggle(c.path)}
                        aria-label={t('git.selectFile', { path: c.path })}
                      />
                      <span className={`${styles.kind} ${styles[c.kind]}`} aria-hidden>
                        {c.kind === 'new'
                          ? 'A'
                          : c.kind === 'deleted'
                            ? 'D'
                            : c.kind === 'renamed'
                              ? 'R'
                              : 'M'}
                      </span>
                      <span className={styles.path}>
                        {dir && <span className={styles.dir}>{dir}</span>}
                        {name}
                      </span>
                    </li>
                  );
                })}
              </ul>
            </div>
          )}

          {changes !== null && changes.length > 0 && (
            <textarea
              className={styles.commitMsg}
              value={message}
              onChange={(e) => setMessage(e.target.value)}
              placeholder={t('git.messagePlaceholder')}
              rows={2}
              aria-label={t('git.messageLabel')}
            />
          )}

          {outcome && <CommitResult outcome={outcome} />}

          <section className={styles.remote}>
            <label className={styles.remoteRow}>
              <span className={styles.remoteLabel}>{t('git.remote')}</span>
              <input
                className={styles.input}
                value={remoteUrl}
                onChange={(e) => setRemoteUrl(e.target.value)}
                placeholder={t('git.remotePlaceholder')}
                spellCheck={false}
              />
            </label>
            <label className={styles.remoteRow}>
              <span className={styles.remoteLabel}>{t('git.token')}</span>
              <input
                className={styles.input}
                type="password"
                value={tokenInput}
                onChange={(e) => setTokenInput(e.target.value)}
                placeholder={connected ? t('git.tokenSaved') : t('git.tokenPlaceholder')}
              />
            </label>
            <div className={styles.remoteActions}>
              <span className={connected ? styles.connected : styles.muted}>
                {connected ? `✓ ${t('git.connected')}` : t('git.notConnected')}
              </span>
              <div className={styles.remoteBtns}>
                {connected && (
                  <button
                    className={styles.secondaryBtn}
                    onClick={() => void clearToken()}
                    aria-label={t('git.clearToken')}
                    title={t('git.clearToken')}
                  >
                    {t('git.clearToken')}
                  </button>
                )}
                <button className={styles.secondaryBtn} onClick={() => void saveRemote()}>
                  {t('git.connect')}
                </button>
              </div>
            </div>
            {remoteError && <p className={styles.errorMsg}>✋ {remoteError}</p>}
            {syncResult && <SyncResultView result={syncResult} />}
            {syncResult?.status === 'merge-required' && (
              <button
                type="button"
                className={styles.commitBtn}
                onClick={() => setResolving(true)}
              >
                {t('conflict.resolve')}
              </button>
            )}
          </section>
        </div>

        <footer className={styles.footer}>
          <button
            className={styles.secondaryBtn}
            onClick={() => void sync()}
            disabled={syncBusy || !remoteUrl.trim()}
          >
            <RefreshCw size={15} aria-hidden />
            {syncBusy ? t('git.syncing') : t('git.syncBtn')}
          </button>
          <button
            className={styles.commitBtn}
            onClick={() => void commit()}
            disabled={busy || !changes || changes.length === 0 || selected.size === 0}
          >
            <GitMerge size={15} aria-hidden />
            {busy
              ? t('git.committing')
              : changes && selected.size < changes.length
                ? t('git.commitSelected', { count: selected.size })
                : t('git.commit')}
          </button>
        </footer>
      </div>
    </div>
    {resolving && (
      <ConflictResolver
        onClose={() => {
          setResolving(false);
          setSyncResult(null);
        }}
      />
    )}
    </>
  );
}

function CommitResult({ outcome }: { outcome: CommitOutcome }) {
  const { t } = useTranslation();
  if (outcome.status === 'committed') {
    return <p className={styles.ok}>✓ {outcome.message}</p>;
  }
  if (outcome.status === 'nothing-to-commit') {
    return <p className={styles.muted}>{t('git.clean')}</p>;
  }
  if (outcome.status === 'error') {
    return <p className={styles.errorMsg}>✋ {outcome.message}</p>;
  }
  return (
    <div className={styles.blocked} role="alert">
      <p className={styles.blockedTitle}>✋ {t('git.blocked')}</p>
      <ul>
        {outcome.findings.map((f) =>
          f.findings.map((s, i) => (
            <li key={`${f.path}:${s.line}:${i}`}>
              <code>{f.path}</code>:{s.line} — {s.kind}
            </li>
          )),
        )}
      </ul>
    </div>
  );
}

function SyncResultView({ result }: { result: SyncResult }) {
  const { t } = useTranslation();
  if (result.status === 'up-to-date') return <p className={styles.muted}>↕ {t('git.upToDate')}</p>;
  if (result.status === 'fast-forward')
    return (
      <div className={`${styles.syncStatus} ${styles.syncStatusSynced}`}>
        <span aria-hidden>↓↑</span>
        <span className={styles.ssText}>{t('git.synced')}</span>
      </div>
    );
  if (result.status === 'merge-required')
    return (
      <div className={`${styles.syncStatus} ${styles.syncStatusConflict}`}>
        <span aria-hidden>⚠</span>
        <span className={styles.ssText}>{t('git.mergeRequired')}</span>
      </div>
    );
  return (
    <div className={`${styles.syncStatus} ${styles.syncStatusError}`}>
      <span aria-hidden>✋</span>
      <span className={styles.ssText}>{result.message}</span>
    </div>
  );
}
