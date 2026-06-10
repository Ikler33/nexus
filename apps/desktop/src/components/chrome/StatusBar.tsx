import { useEffect, useState } from 'react';
import { Check, GitMerge, HardDrive } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import { useJobsStore } from '../../stores/jobs';
import { useSyncStore } from '../../stores/sync';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import styles from './StatusBar.module.css';

/** Дебаунс пере-чтения git-статуса/счётчика заметок по `vault:changed` (события идут пачками). */
const REFRESH_DEBOUNCE_MS = 1500;

/**
 * Нижний status bar по макету `app.jsx` (DP-14): слева — состояние синка (дот + «Синхронизировано»
 * / «Изменения · N», тултип — путь vault) и индексация (анимированный прогресс при активных джобах,
 * иначе «✓ Проиндексировано · N»); справа — конфликт-пилюля (клик → резолвер), Local · UTF-8 ·
 * Markdown. Честная адаптация: прогресса N/M чанков на фронте нет (BACKLOG) — полоска показывает
 * активность очереди джоб.
 */
export function StatusBar() {
  const { t } = useTranslation();
  const info = useVaultStore((s) => s.info);
  const counts = useJobsStore((s) => s.counts);
  const mergeRequired = useSyncStore((s) => s.mergeRequired);
  const conflictFiles = useSyncStore((s) => s.conflictFiles);
  const openConflict = useUIStore((s) => s.openConflict);

  // null — git-статус неизвестен/vault без репо (блок прячем); число — изменённых файлов.
  const [dirty, setDirty] = useState<number | null>(null);
  const [notes, setNotes] = useState<number | null>(null);

  // Подписка: первичная загрузка + пере-чтение по `vault:changed` (правки меняют и git-статус,
  // и счётчик заметок) и по «очередь изменилась» (конец индексации).
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = () => {
      void useJobsStore.getState().refresh();
      tauriApi.git
        .status()
        .then((entries) => setDirty(entries.length))
        .catch(() => setDirty(null));
      tauriApi.vault
        .notesCount()
        .then(setNotes)
        .catch(() => setNotes(null));
    };
    const debounced = () => {
      clearTimeout(timer);
      timer = setTimeout(refresh, REFRESH_DEBOUNCE_MS);
    };
    refresh();
    let offVault = () => {};
    let offJobs = () => {};
    void tauriApi.events.onVaultChanged(debounced).then((fn) => {
      offVault = fn;
    });
    void tauriApi.events.onJobsChanged(debounced).then((fn) => {
      offJobs = fn;
    });
    return () => {
      clearTimeout(timer);
      offVault();
      offJobs();
    };
  }, []);

  const { running, pending, dead } = counts;
  const busy = running > 0 || pending > 0;
  const jobsTitle = t('status.jobsTitle', { running, pending, dead });

  return (
    <div className={styles.statusBar}>
      {/* Состояние синка: чисто → «Синхронизировано», есть правки → «Изменения · N». */}
      {dirty !== null && (
        <span className={styles.item} title={info?.root}>
          <i className={`${styles.dot} ${dirty === 0 ? styles.dotOk : styles.dotWarn}`} aria-hidden />
          {dirty === 0 ? t('status.synced') : t('status.changes', { count: dirty })}
        </span>
      )}

      {/* Индексация: активные джобы → прогресс; иначе «✓ Проиндексировано · N». */}
      {busy ? (
        <span className={`${styles.item} ${styles.jobs}`} title={jobsTitle}>
          <span className={styles.progress} aria-hidden>
            <i />
          </span>
          {t('status.working', { count: running + pending })}
        </span>
      ) : (
        notes !== null && (
          <span className={styles.item}>
            <Check size={12} aria-hidden />
            {t('status.indexed')} · {notes}
          </span>
        )
      )}
      {dead > 0 && (
        <span className={`${styles.item} ${styles.jobsDead}`} title={jobsTitle}>
          ⚠ {dead}
        </span>
      )}

      <div className={styles.right}>
        {mergeRequired && (
          <button
            type="button"
            className={`${styles.item} ${styles.conflict}`}
            onClick={() => openConflict()}
          >
            <GitMerge size={13} aria-hidden />
            {conflictFiles != null && conflictFiles > 0
              ? t('status.conflicts', { count: conflictFiles })
              : t('status.conflict')}
          </button>
        )}
        <span className={styles.item}>
          <HardDrive size={11} aria-hidden />
          {t('status.local')}
        </span>
        <span className={styles.item}>UTF-8</span>
        <span className={styles.item}>Markdown</span>
      </div>
    </div>
  );
}
