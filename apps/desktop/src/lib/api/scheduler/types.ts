/**
 * DTO-типы scheduler-домена (F-2d): сводка очереди фоновых джоб для StatusBar, активные и мёртвые
 * джобы (ADR-007 срез 5). Зеркала Rust-структур (`scheduler::*`) — контракт провода `invoke`.
 * Потребители импортируют по-прежнему из `lib/tauri-api` (barrel-реэкспорт).
 */

/** Сводка очереди планировщика для StatusBar (зеркалит Rust `scheduler::JobCounts`, ADR-007 срез 5). */
export interface JobCounts {
  /** Всего ожидающих (в т.ч. запланированные на будущее recurring) — для тултипа/модалки. */
  pending: number;
  /** Готовы к запуску сейчас (`pending` с наступившим `run_at`) — только это «работа сейчас». */
  ready: number;
  running: number;
  dead: number;
}

/** Активная фоновая джоба (зеркалит Rust `scheduler::ActiveJob`) — модалка очереди за «N задач». */
export interface ActiveJob {
  id: number;
  kind: string;
  state: 'running' | 'pending';
  /** Когда джоба готова к запуску (unix-секунды); для running — момент последнего перехода. */
  runAt: number;
  attempts: number;
}

/** Мёртвая фоновая джоба (зеркалит Rust `scheduler::DeadJob`) — детали для модалки за «⚠ N». */
export interface DeadJob {
  id: number;
  kind: string;
  attempts: number;
  lastError: string | null;
  /** Когда перешла в dead (unix-секунды). */
  updatedAt: number;
}
