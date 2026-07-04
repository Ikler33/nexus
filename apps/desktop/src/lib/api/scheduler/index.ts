import * as mockScheduler from '../../mock/scheduler';
import { bridge, subscribe } from '../bridge';
import type { ActiveJob, DeadJob, JobCounts } from './types';

/**
 * Scheduler-домен (F-2d): фоновый планировщик джоб (ADR-007 срез 5) — счётчики для StatusBar,
 * активные/мёртвые джобы для модалки очереди, повтор/очистка dead, аварийный рестарт воркера (N1) +
 * подписка «очередь изменилась» (`jobs:changed`). Все вызовы — через `bridge` (Tauri ↔ мок
 * `lib/mock/scheduler`); потребители ходят сюда по-прежнему через `tauriApi.scheduler` /
 * `tauriApi.events.onJobsChanged` (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const scheduler = {
  /** Счётчики джоб (pending/running/dead) для индикатора в StatusBar (ADR-007 срез 5). Вне Tauri — нули. */
  counts: (): Promise<JobCounts> =>
    bridge<JobCounts>('get_job_counts', undefined, () => mockScheduler.counts()),

  /** Идёт ли ещё работа над `kind` (pending|running) — для сброса «Генерирую…», когда джоба
   *  завершилась/упала без нового результата. Вне Tauri — `false`. */
  jobActive: (kind: string): Promise<boolean> =>
    bridge<boolean>('job_active', { kind }, () => mockScheduler.jobActive()),

  /** Перезапуск воркера планировщика (N1, аварийная кнопка в модалке очереди). Вне Tauri — no-op. */
  restart: (): Promise<void> =>
    bridge<void>('restart_scheduler', undefined, () => mockScheduler.restart()),

  /** Активные джобы (running/pending) — модалка очереди за «N задач». Вне Tauri — пусто. */
  activeJobs: (): Promise<ActiveJob[]> =>
    bridge<ActiveJob[]>('get_active_jobs', undefined, () => mockScheduler.activeJobs()),

  /** Детали dead-джоб (kind/ошибка/попытки/когда) — модалка за «⚠ N» в StatusBar. Вне Tauri — пусто. */
  deadJobs: (): Promise<DeadJob[]> =>
    bridge<DeadJob[]>('get_dead_jobs', undefined, () => mockScheduler.deadJobs()),

  /** «Повторить» dead-джобу: pending с чистыми attempts. `false` — уже не dead (гонка), не ошибка. */
  retryDead: (id: number): Promise<boolean> =>
    bridge<boolean>('retry_dead_job', { id }, () => mockScheduler.retryDead()),

  /** Удалить все dead-джобы («Очистить» в модалке). Возвращает число удалённых. */
  clearDead: (): Promise<number> =>
    bridge<number>('clear_dead_jobs', undefined, () => mockScheduler.clearDead()),
};

/** Событийная подписка scheduler-домена. Вне Tauri — no-op (мок-бэкенд событий не эмитит). */
export const schedulerEvents = {
  /**
   * Подписка на «очередь задач изменилась» (backend `emit("jobs:changed")` после продуктивного тика
   * воркера — ADR-007). Используется для refetch дайджеста по завершении джобы. Вне Tauri — no-op.
   */
  onJobsChanged: (cb: () => void): Promise<() => void> => subscribe('jobs:changed', () => cb()),
};
