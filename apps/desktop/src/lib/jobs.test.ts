import { describe, expect, it } from 'vitest';

import { isJobReady, selectCurrentRun } from './jobs';
import type { QueueJob } from './jobs';

const NOW = 1_800_000_000_000; // мс
const sec = (ms: number) => Math.floor(ms / 1000);

/** Recurring-джоба следующего суточного прогона («завтрашняя» pending). */
const tomorrow = (kind: string): QueueJob => ({
  id: 7,
  kind,
  state: 'pending',
  runAt: sec(NOW) + 86_400,
});

describe('isJobReady — ready-семантика (NB-4, зеркало Rust has_ready_job)', () => {
  it('КРИТИЧЕСКИЙ: только «завтрашняя» recurring-pending → НЕ готова (steady state не спиннерит)', () => {
    // Именно этот кейс ломал digest/contradictions до NB-4: is_kind_busy возвращал true.
    expect(isJobReady('digest', [tomorrow('digest')], NOW)).toBe(false);
    expect(isJobReady('contradictions', [tomorrow('contradictions')], NOW)).toBe(false);
  });

  it('running-джоба → готова', () => {
    const running: QueueJob = { id: 42, kind: 'digest', state: 'running', runAt: sec(NOW) - 10 };
    expect(isJobReady('digest', [running], NOW)).toBe(true);
  });

  it('pending с наступившим run_at (в прошлом) → готова', () => {
    const ready: QueueJob = { id: 43, kind: 'digest', state: 'pending', runAt: sec(NOW) - 1 };
    expect(isJobReady('digest', [ready], NOW)).toBe(true);
  });

  it('pending с run_at вот-вот наступает (в пределах READY_SLACK_MS = 5с) → считается готовой', () => {
    const soonPending: QueueJob = { id: 44, kind: 'digest', state: 'pending', runAt: sec(NOW) + 4 };
    expect(isJobReady('digest', [soonPending], NOW)).toBe(true);
  });

  it('pending с run_at через 6с (за пределами slack) → не готова', () => {
    const farPending: QueueJob = { id: 45, kind: 'digest', state: 'pending', runAt: sec(NOW) + 6 };
    expect(isJobReady('digest', [farPending], NOW)).toBe(false);
  });

  it('running + «завтрашняя» → готова (running побеждает)', () => {
    const running: QueueJob = { id: 42, kind: 'digest', state: 'running', runAt: sec(NOW) };
    expect(isJobReady('digest', [tomorrow('digest'), running], NOW)).toBe(true);
  });

  it('другой kind не учитывается', () => {
    const running: QueueJob = { id: 10, kind: 'newsfeed', state: 'running', runAt: sec(NOW) };
    expect(isJobReady('digest', [running], NOW)).toBe(false);
  });

  it('пустая очередь → не готова', () => {
    expect(isJobReady('digest', [], NOW)).toBe(false);
  });
});

describe('selectCurrentRun — выбор текущего прогона newsfeed (NB-1, перемещена из news.ts в NB-4)', () => {
  /** Recurring-pending newsfeed «на завтра» — присутствует в КАЖДОМ steady-state снапшоте. */
  const tomorrowNews = tomorrow('newsfeed');

  it('только «завтрашняя» newsfeed-pending → текущего прогона нет', () => {
    expect(selectCurrentRun([tomorrowNews], null, NOW)).toBeUndefined();
  });

  it('running newsfeed на фоне «завтрашней» → текущий прогон', () => {
    const running: QueueJob = { id: 42, kind: 'newsfeed', state: 'running', runAt: sec(NOW) - 10 };
    expect(selectCurrentRun([tomorrowNews, running], null, NOW)?.id).toBe(42);
  });

  it('tracked id держит ретрай-бэкофф (pending с run_at в будущем, но за READY_SLACK_MS)', () => {
    // fail() перекладывает джобу в pending с отложенным run_at — это ещё наш прогон.
    const retrying: QueueJob = { id: 42, kind: 'newsfeed', state: 'pending', runAt: sec(NOW) + 60 };
    expect(selectCurrentRun([tomorrowNews, retrying], 42, NOW)?.id).toBe(42);
  });

  it('tracked id = null, только «завтрашняя» → undefined', () => {
    expect(selectCurrentRun([tomorrowNews], null, NOW)).toBeUndefined();
  });

  it('чужой kind (digest) в очереди не участвует в выборе newsfeed', () => {
    const digestRunning: QueueJob = { id: 9, kind: 'digest', state: 'running', runAt: sec(NOW) };
    expect(selectCurrentRun([digestRunning, tomorrowNews], null, NOW)).toBeUndefined();
  });
});
