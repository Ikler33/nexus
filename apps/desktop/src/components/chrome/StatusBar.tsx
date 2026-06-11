import { useEffect, useState } from 'react';
import { Check, Clock, GitMerge, HardDrive } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { tauriApi } from '../../lib/tauri-api';
import { useJobsStore } from '../../stores/jobs';
import { useSyncStore } from '../../stores/sync';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { DeadJobsModal } from './DeadJobsModal';
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
  // Реальный прогресс полного скана (макет «Индексация N/M»); null — скан не идёт.
  const [indexProg, setIndexProg] = useState<{ done: number; total: number } | null>(null);
  // Модалка деталей dead-джоб (клик по «⚠ N» — отчёт владельца 2026-06-11: ошибки нечем посмотреть).
  const [deadOpen, setDeadOpen] = useState(false);

  useEffect(() => {
    let off = () => {};
    void tauriApi.events
      .onIndexProgress((p) => {
        // Финиш (total,total) или пустой vault — гасим бар (вернётся «Проиндексировано · N»).
        setIndexProg(p.done >= p.total ? null : p);
      })
      .then((fn) => {
        off = fn;
      });
    return () => off();
  }, []);

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
    // Страховочный поллинг (инцидент 2026-06-12): при мёртвом воркере события jobs:changed не
    // приходят и чип застывает («Запланировано» при давно готовых джобах). Раз в минуту — дёшево.
    const poll = setInterval(refresh, 60_000);
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
      clearInterval(poll);
      offVault();
      offJobs();
    };
  }, []);

  const { running, pending, ready, dead } = counts;
  // «Работа сейчас» (анимированный пульс) — только running + готовые к запуску (ready). Запланированные
  // на будущее recurring-джобы (суточные дайджест/лента/противоконфликт) НЕ пульсируют — это вводило в
  // заблуждение «будто что-то работает» (баг 2026-06-11). Их показываем статичным чипом-часами.
  const active = running + ready;
  const scheduled = Math.max(0, pending - ready);
  const busy = active > 0;
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

      {/* Индексация: реальный прогресс скана N/M (макет) → активные джобы → «✓ Проиндексировано · N». */}
      {indexProg ? (
        <span className={`${styles.item} ${styles.jobs}`}>
          <span className={`${styles.progress} ${styles.progressReal}`} aria-hidden>
            <i style={{ width: `${Math.round((indexProg.done / Math.max(1, indexProg.total)) * 100)}%` }} />
          </span>
          {t('status.indexing', { done: indexProg.done, total: indexProg.total })}
        </span>
      ) : busy ? (
        <button
          type="button"
          className={`${styles.item} ${styles.jobs} ${styles.jobsBtn}`}
          title={jobsTitle}
          onClick={() => setDeadOpen(true)}
        >
          <span className={styles.progress} aria-hidden>
            <i />
          </span>
          {t('status.working', { count: active })}
        </button>
      ) : scheduled > 0 ? (
        // Запланированные на будущее джобы есть, но сейчас ничего не выполняется → статичный
        // чип-часы (без пульса), кликабелен → модалка очереди со временем следующего запуска.
        <button
          type="button"
          className={`${styles.item} ${styles.jobsBtn}`}
          title={jobsTitle}
          onClick={() => setDeadOpen(true)}
        >
          <Clock size={12} aria-hidden />
          {t('status.scheduled', { count: scheduled })}
        </button>
      ) : (
        notes !== null && (
          <span className={styles.item}>
            <Check size={12} aria-hidden />
            {t('status.indexed')} · {notes}
          </span>
        )
      )}
      {dead > 0 && (
        <button
          type="button"
          className={`${styles.item} ${styles.jobsDead}`}
          title={jobsTitle}
          onClick={() => setDeadOpen(true)}
        >
          ⚠ {dead}
        </button>
      )}
      {deadOpen && <DeadJobsModal onClose={() => setDeadOpen(false)} />}

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
