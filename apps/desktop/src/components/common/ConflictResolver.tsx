import { useEffect, useMemo, useRef, useState } from 'react';
import { Check, GitMerge } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useFocusTrap } from '../../hooks/useFocusTrap';
import { tauriApi } from '../../lib/tauri-api';
import type { GitConflictFile, GitMergePreview } from '../../lib/tauri-api';
import { useSyncStore } from '../../stores/sync';
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

/** Число строк стороны, отсутствующих в `base` (грубая метрика «правок» этой версии). */
function changedLineCount(side: string | null, base: string | null): number {
  if (side == null) return 0;
  const baseLines = new Set((base ?? '').split('\n'));
  let n = 0;
  for (const ln of side.split('\n')) {
    if (ln.trim() === '') continue; // пустые строки шум не считаем
    if (!baseLines.has(ln)) n++;
  }
  return n;
}

/** Суммарные правки local/remote по всем конфликт-файлам (из существующих conflict-данных). */
function editTotals(files: GitConflictFile[]): { local: number; remote: number } {
  return files.reduce(
    (acc, f) => ({
      local: acc.local + changedLineCount(f.ours, f.base),
      remote: acc.remote + changedLineCount(f.theirs, f.base),
    }),
    { local: 0, remote: 0 },
  );
}

/**
 * 3-way resolver конфликтов merge (Ф4-8 / DP-10, QASR-views по `conflict.jsx`): файловая
 * модель — на каждый файл две кликабельные стороны (выбранная подсвечена, другая тускнеет),
 * кнопки Локально / На диске / Оба, статус-бейдж «не выбрано», ручная правка = выбор `manual`.
 * Раскладка диалога — CSS Grid (header / body+rail / footer). Правый рейл: stats-боксы
 * (суммарные правки local/remote), навигатор по конфликтам (jump+скролл к секции) и bulk-кнопки.
 * Футер: прогресс-бар «N из M» + Cancel/Apply; «Применить» доступно только когда разрешены ВСЕ.
 * Бэкенд атомарен: до «Применить» репозиторий не тронут.
 */
export function ConflictResolver({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const trapRef = useFocusTrap<HTMLDivElement>(onClose); // a11y: Esc/Tab-цикл внутри модалки (audit B10)
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [sides, setSides] = useState<Record<string, Side>>({});
  const [manual, setManual] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [flash, setFlash] = useState<string | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const sectionRefs = useRef<Record<string, HTMLElement | null>>({});

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const p: GitMergePreview = await tauriApi.git.mergePreview();
        if (cancelled) return;
        if (p.status === 'up-to-date') {
          setPhase({ kind: 'none' });
          useSyncStore.getState().setMergeRequired(false);
        } else if (p.status === 'clean') {
          setPhase({ kind: 'clean', theirs: p.theirs });
          useSyncStore.getState().setConflictFiles(0);
        } else {
          setPhase({ kind: 'conflicts', theirs: p.theirs, files: p.files });
          setSides(Object.fromEntries(p.files.map((f) => [f.path, null])));
          // DP-14: число для конфликт-пилюли статусбара.
          useSyncStore.getState().setConflictFiles(p.files.length);
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
  const stats = useMemo(
    () => (phase.kind === 'conflicts' ? editTotals(phase.files) : { local: 0, remote: 0 }),
    [phase],
  );

  const pick = (path: string, side: Side) => setSides((s) => ({ ...s, [path]: side }));
  const pickAll = (side: Side) => {
    if (phase.kind !== 'conflicts') return;
    setSides(Object.fromEntries(phase.files.map((f) => [f.path, side])));
  };

  // Навигатор: скролл к секции файла + кратковременная flash-подсветка.
  const jump = (path: string) => {
    const el = sectionRefs.current[path];
    const sc = bodyRef.current;
    if (el && sc) sc.scrollTop = el.offsetTop - 16;
    setFlash(path);
    window.setTimeout(() => setFlash((f) => (f === path ? null : f)), 1100);
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
      // Merge закрыт — конфликт-пилюля статусбара гаснет (DP-14).
      useSyncStore.getState().setMergeRequired(false);
    } catch (e) {
      setPhase({ kind: 'error', message: String(e) });
    } finally {
      setBusy(false);
    }
  };

  const isConflicts = phase.kind === 'conflicts';

  return (
    <div className={styles.backdrop} onClick={onClose} role="presentation">
      <div
        ref={trapRef}
        tabIndex={-1}
        className={styles.dialog}
        role="dialog"
        aria-modal="true"
        aria-label={t('conflict.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <div className={styles.headIcoBox} aria-hidden>
            <GitMerge size={20} />
          </div>
          <div className={styles.headText}>
            <div className={styles.title}>{t('conflict.title')}</div>
            <div className={styles.subtitle}>{t('conflict.subtitle')}</div>
          </div>
          <button type="button" className={styles.close} onClick={onClose} aria-label={t('git.close')}>
            ✕
          </button>
        </header>

        <div className={styles.body} ref={bodyRef}>
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

          {phase.kind === 'conflicts' &&
            phase.files.map((f, idx) => {
              const side = sides[f.path] ?? null;
              return (
                <section
                  key={f.path}
                  className={`${styles.file} ${flash === f.path ? styles.flash : ''}`}
                  ref={(el) => {
                    sectionRefs.current[f.path] = el;
                  }}
                >
                  <div className={styles.fileHead}>
                    <GitMerge size={13} aria-hidden />
                    <span className={styles.hunkNum}>{t('conflict.hunk', { n: idx + 1 })}</span>
                    <span className={styles.filePath}>{f.path}</span>
                    <span
                      className={`${styles.stateBadge} ${side ? styles.stateDone : styles.statePending}`}
                    >
                      {side ? t(`conflict.side.${side}`) : t('conflict.unresolved')}
                    </span>
                  </div>
                  <div className={styles.compare}>
                    <button
                      type="button"
                      className={`${styles.side} ${styles.sideOurs} ${side === 'ours' ? styles.chosen : ''} ${side && side !== 'ours' ? styles.dimmed : ''}`}
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
                      className={`${styles.side} ${styles.sideTheirs} ${side === 'theirs' ? styles.chosen : ''} ${side && side !== 'theirs' ? styles.dimmed : ''}`}
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
                      className={`${styles.pickBtn} ${side === 'ours' ? styles.pickOnLocal : ''}`}
                      onClick={() => pick(f.path, 'ours')}
                    >
                      {t('conflict.ours')}
                    </button>
                    <button
                      type="button"
                      className={`${styles.pickBtn} ${side === 'theirs' ? styles.pickOnRemote : ''}`}
                      onClick={() => pick(f.path, 'theirs')}
                    >
                      {t('conflict.theirs')}
                    </button>
                    <button
                      type="button"
                      className={`${styles.pickBtn} ${side === 'both' ? styles.pickOnBoth : ''}`}
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
        </div>

        {isConflicts && (
          <aside className={styles.rail} aria-label={t('conflict.navAria')}>
            <div className={styles.railScroll}>
              <div className={styles.stat}>
                <div className={`${styles.statBox} ${styles.statLocal}`}>
                  <div className={styles.statValue}>{stats.local}</div>
                  <div className={styles.statLabel}>{t('conflict.localEdits')}</div>
                </div>
                <div className={`${styles.statBox} ${styles.statRemote}`}>
                  <div className={styles.statValue}>{stats.remote}</div>
                  <div className={styles.statLabel}>{t('conflict.remoteEdits')}</div>
                </div>
              </div>

              <div>
                <div className={styles.railHeading}>
                  {t('conflict.conflictsHeading')} · {resolvedCount}/{total}
                </div>
                <div className={styles.jumpList}>
                  {phase.files.map((f, idx) => {
                    const c = sides[f.path] ?? null;
                    return (
                      <button
                        type="button"
                        key={f.path}
                        className={`${styles.jump} ${c ? styles.jumpResolved : ''}`}
                        onClick={() => jump(f.path)}
                      >
                        <span className={styles.jumpDot} aria-hidden />
                        <span className={styles.jumpTitle}>{t('conflict.hunk', { n: idx + 1 })}</span>
                        <span className={styles.jumpPath}>{f.path}</span>
                      </button>
                    );
                  })}
                </div>
              </div>

              <div className={styles.bulk}>
                <div className={styles.railHeading}>{t('conflict.chooseHeading')}</div>
                <button type="button" className={styles.bulkBtn} onClick={() => pickAll('ours')}>
                  <span className={`${styles.swatch} ${styles.swatchLocal}`} aria-hidden />
                  {t('conflict.allOurs')}
                </button>
                <button type="button" className={styles.bulkBtn} onClick={() => pickAll('theirs')}>
                  <span className={`${styles.swatch} ${styles.swatchRemote}`} aria-hidden />
                  {t('conflict.allTheirs')}
                </button>
              </div>
            </div>
          </aside>
        )}

        {isConflicts && (
          <footer className={styles.footer}>
            <div className={styles.progress}>
              <div className={styles.progressBar}>
                <i style={{ width: `${total ? (resolvedCount / total) * 100 : 0}%` }} />
              </div>
              <span className={styles.progressLabel}>
                {t('conflict.progress', { done: resolvedCount, total })}
              </span>
            </div>
            <button type="button" className={styles.cancelBtn} onClick={onClose}>
              {t('conflict.cancel')}
            </button>
            <button
              type="button"
              className={styles.applyBtn}
              disabled={busy || !allResolved}
              title={allResolved ? undefined : t('conflict.resolveAllFirst')}
              onClick={() => phase.kind === 'conflicts' && void apply(phase.theirs, phase.files)}
            >
              {busy ? t('conflict.applying') : t('conflict.apply')}
            </button>
          </footer>
        )}
      </div>
    </div>
  );
}
