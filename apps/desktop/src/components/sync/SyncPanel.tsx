import { useCallback, useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import {
  type GitCommitOutcome,
  type GitPullOutcome,
  type GitStatusEntry,
  tauriApi,
} from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { ConflictResolver } from './ConflictResolver';
import styles from './SyncPanel.module.css';

type SyncResult = GitPullOutcome | { status: 'error'; message: string };

/**
 * Панель синхронизации (Ф3, git-sync): изменения рабочего дерева + коммит (secret-scan на бэке),
 * настройка remote (URL + токен в системный keychain) и sync (pull-ff → push). Конфликт
 * (`merge-required`) пока только сигналим — ручное разрешение в BACKLOG (завязано на marketplace).
 */
export function SyncPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeSync);
  const [changes, setChanges] = useState<GitStatusEntry[] | null>(null);
  const [outcome, setOutcome] = useState<GitCommitOutcome | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');

  // remote-конфиг
  const [remoteUrl, setRemoteUrl] = useState('');
  const [tokenInput, setTokenInput] = useState('');
  const [connected, setConnected] = useState(false);
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null);
  const [syncBusy, setSyncBusy] = useState(false);
  const [resolving, setResolving] = useState(false);

  const load = useCallback(() => {
    setOutcome(null);
    setChanges(null);
    tauriApi.git
      .status()
      .then(setChanges)
      .catch(() => setChanges([]));
    tauriApi.git
      .getRemote()
      .then((r) => setRemoteUrl(r ?? ''))
      .catch(() => {});
    tauriApi.git
      .hasToken()
      .then(setConnected)
      .catch(() => {});
  }, []);
  useEffect(() => load(), [load]);

  const commit = async () => {
    setBusy(true);
    try {
      const o = await tauriApi.git.commit(message.trim() || undefined);
      if (o.status === 'committed') setMessage('');
      setOutcome(o);
      if (o.status !== 'blocked-by-secrets') {
        await tauriApi.git
          .status()
          .then(setChanges)
          .catch(() => setChanges([]));
      }
    } catch {
      /* ошибки команды не ломают UI */
    } finally {
      setBusy(false);
    }
  };

  const saveRemote = async () => {
    try {
      if (remoteUrl.trim()) await tauriApi.git.setRemote(remoteUrl.trim());
      if (tokenInput.trim()) {
        await tauriApi.git.setToken(tokenInput.trim());
        setTokenInput('');
      }
      setConnected(await tauriApi.git.hasToken());
    } catch {
      /* ignore */
    }
  };

  const sync = async () => {
    setSyncBusy(true);
    setSyncResult(null);
    try {
      setSyncResult(await tauriApi.git.sync());
      await tauriApi.git
        .status()
        .then(setChanges)
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
            <ul className={styles.changes} aria-label={t('git.changes')}>
              {changes.map((c) => (
                <li key={c.path} className={styles.change}>
                  <span className={`${styles.kind} ${styles[c.kind]}`} aria-hidden>
                    {c.kind === 'new'
                      ? 'A'
                      : c.kind === 'deleted'
                        ? 'D'
                        : c.kind === 'renamed'
                          ? 'R'
                          : 'M'}
                  </span>
                  <span className={styles.path}>{c.path}</span>
                </li>
              ))}
            </ul>
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
              <button className={styles.secondaryBtn} onClick={() => void saveRemote()}>
                {t('git.connect')}
              </button>
            </div>
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
            {syncBusy ? t('git.syncing') : t('git.syncBtn')}
          </button>
          <button
            className={styles.commitBtn}
            onClick={() => void commit()}
            disabled={busy || !changes || changes.length === 0}
          >
            {busy ? t('git.committing') : t('git.commit')}
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

function CommitResult({ outcome }: { outcome: GitCommitOutcome }) {
  const { t } = useTranslation();
  if (outcome.status === 'committed') {
    return <p className={styles.ok}>✓ {outcome.message}</p>;
  }
  if (outcome.status === 'nothing-to-commit') {
    return <p className={styles.muted}>{t('git.clean')}</p>;
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
  if (result.status === 'fast-forward') return <p className={styles.ok}>↓↑ {t('git.synced')}</p>;
  if (result.status === 'merge-required')
    return <p className={styles.warn}>⚠ {t('git.mergeRequired')}</p>;
  return <p className={styles.errorMsg}>✋ {result.message}</p>;
}
