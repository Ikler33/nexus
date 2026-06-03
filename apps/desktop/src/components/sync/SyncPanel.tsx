import { useCallback, useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { type GitCommitOutcome, type GitStatusEntry, tauriApi } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import styles from './SyncPanel.module.css';

/**
 * Панель синхронизации (Ф3, git-sync): показывает изменения рабочего дерева и коммитит их через
 * `git_commit` (secret-scan на бэке — при находке секрета коммит блокируется). pull/push — Ф3-3b.
 */
export function SyncPanel() {
  const { t } = useTranslation();
  const close = useUIStore((s) => s.closeSync);
  const [changes, setChanges] = useState<GitStatusEntry[] | null>(null);
  const [outcome, setOutcome] = useState<GitCommitOutcome | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    setOutcome(null);
    setChanges(null);
    tauriApi.git
      .status()
      .then(setChanges)
      .catch(() => setChanges([]));
  }, []);
  useEffect(() => load(), [load]);

  const commit = async () => {
    setBusy(true);
    try {
      const o = await tauriApi.git.commit();
      setOutcome(o);
      // Обновляем список файлов (после коммита обычно пусто), но СОХРАНЯЕМ сообщение об исходе.
      if (o.status !== 'blocked-by-secrets') {
        await tauriApi.git
          .status()
          .then(setChanges)
          .catch(() => setChanges([]));
      }
    } catch {
      /* ошибки команды показываем как пустой исход — UI остаётся рабочим */
    } finally {
      setBusy(false);
    }
  };

  return (
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

          {outcome && <Outcome outcome={outcome} />}
        </div>

        <footer className={styles.footer}>
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
  );
}

function Outcome({ outcome }: { outcome: GitCommitOutcome }) {
  const { t } = useTranslation();
  if (outcome.status === 'committed') {
    return <p className={styles.ok}>✓ {outcome.message}</p>;
  }
  if (outcome.status === 'nothing-to-commit') {
    return <p className={styles.muted}>{t('git.clean')}</p>;
  }
  // blocked-by-secrets
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
