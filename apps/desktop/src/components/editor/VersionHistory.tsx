import { useCallback, useEffect, useMemo, useState } from 'react';
import { History, RotateCcw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { diffStat, lineDiff } from '../../lib/diff';
import { relTime } from '../../lib/time';
import { tauriApi } from '../../lib/tauri-api';
import { activeBuffer, useWorkspaceStore } from '../../stores/workspace';
import styles from './VersionHistory.module.css';

type SnapshotMeta = { ts: number; size: number };
/** Опорная версия для сравнения/восстановления: текущий файл на диске ИЛИ снапшот по `ts`. */
type Ref = { kind: 'disk' } | { kind: 'snapshot'; ts: number };

/**
 * История версий заметки (SAFE-6): слева — список снапшотов (+ «Текущий на диске», если в буфере
 * есть несохранённое), справа — diff «выбранная версия → сейчас» и кнопка «Восстановить».
 * Diff читается как превью восстановления: `−` строки уйдут, `+` вернутся.
 */
export function VersionHistory({ onClose }: { onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const active = useWorkspaceStore(activeBuffer);
  const reloadFromDisk = useWorkspaceStore((s) => s.reloadFromDisk);

  const path = active?.path ?? null;
  const current = active?.doc ?? '';
  const dirty = Boolean(active?.dirty);
  const [snaps, setSnaps] = useState<SnapshotMeta[]>([]);
  const [sel, setSel] = useState<Ref | null>(null);
  const [refContent, setRefContent] = useState('');
  const [loading, setLoading] = useState(true);

  // Esc закрывает модалку (capture, чтобы перехватить раньше глобального reading-Esc).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onClose]);

  // Список версий при открытии: грязный буфер → дефолт сравнения с диском; иначе с новейшим снапшотом.
  useEffect(() => {
    if (!path) {
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    tauriApi.vault
      .listVersions(path)
      .then((list) => {
        if (cancelled) return;
        setSnaps(list);
        if (dirty) setSel({ kind: 'disk' });
        else if (list.length) setSel({ kind: 'snapshot', ts: list[0].ts });
        else setSel(null);
      })
      .catch(() => {
        if (!cancelled) setSnaps([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  // Содержимое выбранной опорной версии (диск или снапшот).
  useEffect(() => {
    if (!path || !sel) {
      setRefContent('');
      return;
    }
    let cancelled = false;
    const load =
      sel.kind === 'disk'
        ? tauriApi.vault.readFileMeta(path).then((m) => m.content)
        : tauriApi.vault.readVersion(path, sel.ts);
    load
      .then((c) => {
        if (!cancelled) setRefContent(c);
      })
      .catch(() => {
        if (!cancelled) setRefContent('');
      });
    return () => {
      cancelled = true;
    };
  }, [path, sel]);

  // diff «сейчас → выбранная версия»: del = строка уйдёт при восстановлении, add = вернётся.
  const diff = useMemo(() => lineDiff(current, refContent), [current, refContent]);
  const stat = useMemo(() => diffStat(diff), [diff]);
  const identical = stat.added === 0 && stat.removed === 0;

  const restore = useCallback(async () => {
    if (!path || !sel) return;
    await tauriApi.vault.writeFile(path, refContent, true); // ручная точка истории
    await reloadFromDisk(path); // синхронизируем буфер с восстановленным содержимым
    onClose();
  }, [path, sel, refContent, reloadFromDisk, onClose]);

  return (
    <div className={styles.backdrop} onClick={onClose} role="presentation">
      <div
        className={styles.dialog}
        role="dialog"
        aria-modal="true"
        aria-label={t('versions.title')}
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          <History size={16} aria-hidden />
          <span className={styles.title}>{t('versions.title')}</span>
          {path && <span className={styles.path}>{path}</span>}
          <button className={styles.close} onClick={onClose} aria-label={t('versions.close')}>
            <X size={14} aria-hidden />
          </button>
        </header>

        {!path ? (
          <p className={styles.empty}>{t('versions.noFile')}</p>
        ) : (
          <div className={styles.body}>
            <ul className={styles.list}>
              {dirty && (
                <li>
                  <button
                    className={`${styles.item} ${sel?.kind === 'disk' ? styles.itemActive : ''}`}
                    onClick={() => setSel({ kind: 'disk' })}
                  >
                    <span className={styles.itemMain}>{t('versions.onDisk')}</span>
                    <span className={styles.itemSub}>{t('versions.unsavedHint')}</span>
                  </button>
                </li>
              )}
              {loading ? (
                <li className={styles.muted}>{t('versions.loading')}</li>
              ) : snaps.length === 0 ? (
                <li className={styles.muted}>{t('versions.none')}</li>
              ) : (
                snaps.map((s) => (
                  <li key={s.ts}>
                    <button
                      className={`${styles.item} ${
                        sel?.kind === 'snapshot' && sel.ts === s.ts ? styles.itemActive : ''
                      }`}
                      onClick={() => setSel({ kind: 'snapshot', ts: s.ts })}
                    >
                      <span className={styles.itemMain}>
                        {relTime(Math.floor(s.ts / 1000), i18n.language)}
                      </span>
                      <span className={styles.itemSub}>{t('versions.bytes', { size: s.size })}</span>
                    </button>
                  </li>
                ))
              )}
            </ul>

            <div className={styles.diffPane}>
              {sel ? (
                <>
                  <div className={styles.diffHead}>
                    <span className={styles.statAdd}>+{stat.added}</span>
                    <span className={styles.statDel}>−{stat.removed}</span>
                    <span className={styles.diffNote}>{t('versions.restoreHint')}</span>
                    <button
                      className={styles.restore}
                      onClick={() => void restore()}
                      disabled={identical}
                    >
                      <RotateCcw size={13} aria-hidden /> {t('versions.restore')}
                    </button>
                  </div>
                  <div className={styles.diff}>
                    {identical ? (
                      <p className={styles.muted}>{t('versions.identical')}</p>
                    ) : (
                      diff.map((d, idx) => (
                        <div
                          key={idx}
                          className={`${styles.line} ${
                            d.type === 'add' ? styles.add : d.type === 'del' ? styles.del : ''
                          }`}
                        >
                          <span className={styles.gutter}>
                            {d.type === 'add' ? '+' : d.type === 'del' ? '−' : ''}
                          </span>
                          <span className={styles.lineText}>{d.text || ' '}</span>
                        </div>
                      ))
                    )}
                  </div>
                </>
              ) : (
                <p className={styles.empty}>{t('versions.pickHint')}</p>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
