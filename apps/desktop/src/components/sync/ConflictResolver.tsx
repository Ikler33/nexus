import { useEffect, useMemo, useState } from 'react';
import { Check, GitMerge } from 'lucide-react';
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

/** Выбор стороны конфликта (DP-10, макет conflict.jsx): null = ещё не разрешён. */
type Side = 'ours' | 'theirs' | 'both' | 'manual' | null;

function sideContent(f: GitConflictFile, side: Side): string {
  switch (side) {
    case 'ours':
      return f.ours ?? '';
    case 'theirs':
      return f.theirs ?? '';
    case 'both':
      return [f.ours, f.theirs].filter((v): v is string => v != null).join('\n');
    default:
      return f.ours ?? f.theirs ?? f.base ?? '';
  }
}

/**
 * 3-way resolver конфликтов merge (Ф4-8 / DP-10 по `conflict.jsx`): на каждый файл — две
 * кликабельные стороны (выбранная подсвечена, другая тускнеет), кнопки Локально / На диске /
 * Оба, статус-бейдж «не выбрано», прогресс «Разрешено N из M» и bulk-кнопки; «Применить»
 * доступно только когда разрешены ВСЕ. Ручная правка результата = выбор `manual`.
 * Бэкенд атомарен: до «Применить» репозиторий не тронут.
 */
export function ConflictResolver({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [sides, setSides] = useState<Record<string, Side>>({});
  const [manual, setManual] = useState<Record<string, string>>({});
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
          setSides(Object.fromEntries(p.files.map((f) => [f.path, null])));
        }
      } catch (e) {
        if (!cancelled) setPhase({ kind: 'error', message: String(e) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const resolvedCount = useMemo(
    () => Object.values(sides).filter((s) => s != null).length,
    [sides],
  );
  const total = phase.kind === 'conflicts' ? phase.files.length : 0;
  const allResolved = phase.kind === 'conflicts' && resolvedCount === total;

  const pick = (path: string, side: Side) => setSides((s) => ({ ...s, [path]: side }));
  const pickAll = (side: Side) => {
    if (phase.kind !== 'conflicts') return;
    setSides(Object.fromEntries(phase.files.map((f) => [f.path, side])));
  };

  const resolutionFor = (f: GitConflictFile): string => {
    const side = sides[f.path];
    if (side === 'manual') return manual[f.path] ?? '';
    return sideContent(f, side);
  };

  const apply = async (theirs: string, files: GitConflictFile[]) => {
    setBusy(true);
    try {
      const res = files.map((f) => [f.path, resolutionFor(f)] as [string, string]);
      const oid = await tauriApi.git.resolveConflicts(theirs, res);
      setPhase({ kind: 'done', oid });
    } catch (e) {
      setPhase({ kind: 'error', message: String(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.backdrop} onClick={onClose} role="presentation">
      <div
        className={styles.dialog}
        role="dialog"
        aria-label={t('conflict.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <GitMerge size={16} className={styles.headIco} aria-hidden />
          <span className={styles.title}>{t('conflict.title')}</span>
          {phase.kind === 'conflicts' && (
            <span className={styles.progressLabel}>
              {t('conflict.progress', { done: resolvedCount, total })}
            </span>
          )}
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
              <div className={styles.bulk}>
                <button type="button" className={styles.bulkBtn} onClick={() => pickAll('ours')}>
                  {t('conflict.allOurs')}
                </button>
                <button type="button" className={styles.bulkBtn} onClick={() => pickAll('theirs')}>
                  {t('conflict.allTheirs')}
                </button>
              </div>
              {phase.files.map((f, idx) => {
                const side = sides[f.path] ?? null;
                return (
                  <section key={f.path} className={styles.file}>
                    <div className={styles.fileHead}>
                      <GitMerge size={13} aria-hidden />
                      <span className={styles.hunkNum}>
                        {t('conflict.hunk', { n: idx + 1 })}
                      </span>
                      <span className={styles.filePath}>{f.path}</span>
                      <span
                        className={`${styles.stateBadge} ${side ? styles.stateDone : styles.statePending}`}
                      >
                        {side
                          ? t(`conflict.side.${side}`)
                          : t('conflict.unresolved')}
                      </span>
                    </div>
                    <div className={styles.compare}>
                      <button
                        type="button"
                        className={`${styles.side} ${side === 'ours' ? styles.chosen : ''} ${side && side !== 'ours' ? styles.dimmed : ''}`}
                        onClick={() => pick(f.path, 'ours')}
                      >
                        <span className={styles.sideLabel}>
                          {t('conflict.ours')}
                          {side === 'ours' && <Check size={12} className={styles.checkIco} aria-hidden />}
                        </span>
                        <pre className={styles.sideText}>{f.ours ?? '∅'}</pre>
                      </button>
                      <button
                        type="button"
                        className={`${styles.side} ${side === 'theirs' ? styles.chosen : ''} ${side && side !== 'theirs' ? styles.dimmed : ''}`}
                        onClick={() => pick(f.path, 'theirs')}
                      >
                        <span className={styles.sideLabel}>
                          {t('conflict.theirs')}
                          {side === 'theirs' && (
                            <Check size={12} className={styles.checkIco} aria-hidden />
                          )}
                        </span>
                        <pre className={styles.sideText}>{f.theirs ?? '∅'}</pre>
                      </button>
                    </div>
                    <div className={styles.pick}>
                      <button
                        type="button"
                        className={`${styles.pickBtn} ${side === 'ours' ? styles.pickOn : ''}`}
                        onClick={() => pick(f.path, 'ours')}
                      >
                        {t('conflict.ours')}
                      </button>
                      <button
                        type="button"
                        className={`${styles.pickBtn} ${side === 'theirs' ? styles.pickOn : ''}`}
                        onClick={() => pick(f.path, 'theirs')}
                      >
                        {t('conflict.theirs')}
                      </button>
                      <button
                        type="button"
                        className={`${styles.pickBtn} ${side === 'both' ? styles.pickOn : ''}`}
                        onClick={() => pick(f.path, 'both')}
                      >
                        {t('conflict.both')}
                      </button>
                    </div>
                    <textarea
                      className={styles.result}
                      value={side === 'manual' ? (manual[f.path] ?? '') : resolutionFor(f)}
                      onChange={(e) => {
                        setManual((m) => ({ ...m, [f.path]: e.target.value }));
                        pick(f.path, 'manual');
                      }}
                      aria-label={`${t('conflict.result')}: ${f.path}`}
                    />
                  </section>
                );
              })}
              <button
                type="button"
                className={styles.applyBtn}
                disabled={busy || !allResolved}
                title={allResolved ? undefined : t('conflict.resolveAllFirst')}
                onClick={() => void apply(phase.theirs, phase.files)}
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
