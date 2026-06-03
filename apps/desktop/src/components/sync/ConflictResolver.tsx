import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import type { GitConflictFile, GitMergePreview } from '../../lib/tauri-api';
import styles from './ConflictResolver.module.css';

type Phase =
  | { kind: 'loading' }
  | { kind: 'none' }
  | { kind: 'clean'; theirs: string }
  | { kind: 'conflicts'; theirs: string; files: GitConflictFile[] }
  | { kind: 'error'; message: string }
  | { kind: 'done'; oid: string };

/**
 * 3-way resolver конфликтов merge (Ф4-8). Грузит `git.mergePreview` (in-memory, безопасно),
 * показывает на каждый файл наше/их + редактируемый результат (дефолт — наше), применяет через
 * `git.resolveConflicts` (merge-коммит + push). Бэкенд атомарен: до «Применить» репозиторий не тронут.
 */
export function ConflictResolver({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [resolutions, setResolutions] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const p: GitMergePreview = await tauriApi.git.mergePreview();
        if (cancelled) return;
        if (p.status === 'up-to-date') {
          setPhase({ kind: 'none' });
        } else if (p.status === 'clean') {
          setPhase({ kind: 'clean', theirs: p.theirs });
        } else {
          setPhase({ kind: 'conflicts', theirs: p.theirs, files: p.files });
          const init: Record<string, string> = {};
          for (const f of p.files) init[f.path] = f.ours ?? f.theirs ?? f.base ?? '';
          setResolutions(init);
        }
      } catch (e) {
        if (!cancelled) setPhase({ kind: 'error', message: String(e) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const apply = async (theirs: string, res: [string, string][]) => {
    setBusy(true);
    try {
      const oid = await tauriApi.git.resolveConflicts(theirs, res);
      setPhase({ kind: 'done', oid });
    } catch (e) {
      setPhase({ kind: 'error', message: String(e) });
    } finally {
      setBusy(false);
    }
  };

  const pick = (path: string, content: string | null) =>
    setResolutions((r) => ({ ...r, [path]: content ?? '' }));

  return (
    <div className={styles.backdrop} onClick={onClose} role="presentation">
      <div
        className={styles.dialog}
        role="dialog"
        aria-label={t('conflict.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <span className={styles.title}>{t('conflict.title')}</span>
          <button type="button" className={styles.close} onClick={onClose} aria-label={t('git.close')}>
            ✕
          </button>
        </header>
        <div className={styles.body}>
          {phase.kind === 'loading' && <p className={styles.muted}>{t('git.loading')}</p>}
          {phase.kind === 'none' && <p className={styles.muted}>↕ {t('conflict.upToDate')}</p>}
          {phase.kind === 'error' && <p className={styles.errorMsg}>✋ {phase.message}</p>}
          {phase.kind === 'done' && (
            <p className={styles.ok}>
              ✓ {t('conflict.done')} <code>{phase.oid.slice(0, 9)}</code>
            </p>
          )}

          {phase.kind === 'clean' && (
            <>
              <p className={styles.muted}>{t('conflict.clean')}</p>
              <button
                type="button"
                className={styles.applyBtn}
                disabled={busy}
                onClick={() => void apply(phase.theirs, [])}
              >
                {busy ? t('conflict.applying') : t('conflict.applyClean')}
              </button>
            </>
          )}

          {phase.kind === 'conflicts' && (
            <>
              {phase.files.map((f) => (
                <section key={f.path} className={styles.file}>
                  <div className={styles.fileHead}>
                    <span className={styles.filePath}>{f.path}</span>
                    <div className={styles.pick}>
                      <button type="button" className={styles.pickBtn} onClick={() => pick(f.path, f.ours)}>
                        {t('conflict.ours')}
                      </button>
                      <button type="button" className={styles.pickBtn} onClick={() => pick(f.path, f.theirs)}>
                        {t('conflict.theirs')}
                      </button>
                    </div>
                  </div>
                  <div className={styles.compare}>
                    <pre className={styles.side}>
                      <span className={styles.sideLabel}>{t('conflict.ours')}</span>
                      {f.ours ?? '∅'}
                    </pre>
                    <pre className={styles.side}>
                      <span className={styles.sideLabel}>{t('conflict.theirs')}</span>
                      {f.theirs ?? '∅'}
                    </pre>
                  </div>
                  <textarea
                    className={styles.result}
                    value={resolutions[f.path] ?? ''}
                    onChange={(e) =>
                      setResolutions((r) => ({ ...r, [f.path]: e.target.value }))
                    }
                    aria-label={`${t('conflict.result')}: ${f.path}`}
                  />
                </section>
              ))}
              <button
                type="button"
                className={styles.applyBtn}
                disabled={busy}
                onClick={() =>
                  void apply(
                    phase.theirs,
                    phase.files.map((f) => [f.path, resolutions[f.path] ?? ''] as [string, string]),
                  )
                }
              >
                {busy ? t('conflict.applying') : t('conflict.apply')}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
