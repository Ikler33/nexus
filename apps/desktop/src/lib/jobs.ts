/**
 * Общие примитивы очереди планировщика (NB-4): ready-семантика джоб.
 *
 * Основная проблема (`is_kind_busy`-футган): Rust `is_kind_busy` / frontend `jobActive` считают
 * «занятыми» ЛЮБЫЕ pending/running-джобы, включая recurring-pending «на завтра» (reschedule_if_absent
 * после каждого прогона). Для recurring-kinds (digest, contradictions, newsfeed, …) это означает, что
 * в steady state `jobActive` возвращает `true` всегда → вечный спиннер/задизейбленная кнопка.
 *
 * Решение: ready-семантика (зеркалит Rust `has_ready_job`, scheduler.rs:426) — считаем «работой
 * сейчас» только `running` ИЛИ `pending` с наступившим `run_at` (± `READY_SLACK_MS`). «Завтрашняя»
 * recurring-pending с `run_at = now + 86400` этот фильтр не проходит.
 *
 * ⚠️ Правило для recurring-kinds: НИКОГДА не используй `tauriApi.scheduler.jobActive` для
 * определения «идёт ли прогон». Используй `isJobReady` + `tauriApi.scheduler.activeJobs`.
 */

/** Мини-форма активной джобы — структурное подмножество `ActiveJob` (lib/tauri-api.ts). */
export interface QueueJob {
  id: number;
  kind: string;
  state: 'running' | 'pending';
  /** Unix-СЕКУНДЫ (как в `ActiveJob`). */
  runAt: number;
}

/**
 * Зазор «pending вот-вот стартует»: джоба, чей `run_at` наступает в пределах ближайшего опроса
 * (5 с = POLL_MS из news.ts), уже считается «готовой» сейчас. Только в БУДУЩЕЕ: отсекает
 * «завтрашнюю» recurring-pending (run_at + 86400 с), но захватывает немедленно-стартующую.
 */
export const READY_SLACK_MS = 5_000;

/** Единственный источник ready-семантики: `running` ИЛИ `pending` с наступившим (±слак) `run_at`. */
function readyNow(j: QueueJob, now: number): boolean {
  return j.state === 'running' || j.runAt * 1000 <= now + READY_SLACK_MS;
}

/**
 * Есть ли в очереди ГОТОВАЯ (работает сейчас или вот-вот стартует) джоба указанного `kind`?
 *
 * Зеркалит Rust `has_ready_job` (scheduler.rs:426): `running` ИЛИ `pending` с наступившим
 * `run_at` (± `READY_SLACK_MS`).
 *
 * ⚠️ Используй ЭТУ функцию (не `jobActive` / `is_kind_busy`) для recurring-kinds
 * (digest, contradictions, newsfeed, episode_rollup, stale_radar, home_widget:*).
 * `jobActive` в steady state вечно возвращает `true` для recurring-kinds — корень NB-4 бага.
 *
 * @example
 * ```ts
 * const active = await tauriApi.scheduler.activeJobs();
 * if (!isJobReady('digest', active, Date.now())) stillGenerating = false;
 * ```
 */
export function isJobReady(kind: string, active: QueueJob[], now: number): boolean {
  return active.some(
    (j) => j.kind === kind && readyNow(j, now),
  );
}

/**
 * Выбирает из очереди джобу ТЕКУЩЕГО прогона `newsfeed`, отфильтровывая «завтрашнюю»
 * recurring-pending. Семантика зеркалит Rust `has_ready_job`: `running` ИЛИ `pending` с
 * наступившим (± `READY_SLACK_MS`) `run_at`. Если прогон уже отслеживается (`trackedId`),
 * держимся ЕГО id — так ретрай-бэкофф (pending с `run_at` в будущем) не теряется.
 *
 * Перемещена из `stores/news.ts` в NB-4; `news.ts` реэкспортирует для обратной совместимости.
 */
export function selectCurrentRun(
  active: QueueJob[],
  trackedId: number | null,
  now: number,
): QueueJob | undefined {
  const news = active.filter((j) => j.kind === 'newsfeed');
  if (trackedId !== null) return news.find((j) => j.id === trackedId);
  return news.find((j) => readyNow(j, now));
}
