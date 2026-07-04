import type { ActiveJob, DeadJob, JobCounts } from '../tauri-api';

/**
 * Мок scheduler-домена для браузерного превью / vitest (вне Tauri): фонового планировщика джоб нет —
 * очередь пуста, ничего не «работает», перезапуск/повтор/очистка — no-op. Зеркалит контракт Rust-команд
 * `scheduler::*` на пустой очереди (mock-must-match-backend). Инлайн-заглушки переехали из tauri-api.ts
 * (ratchet parity-гейта (в), F-2d).
 */
export async function counts(): Promise<JobCounts> {
  return { pending: 0, ready: 0, running: 0, dead: 0 };
}

export async function jobActive(): Promise<boolean> {
  return false;
}

export async function restart(): Promise<void> {}

export async function activeJobs(): Promise<ActiveJob[]> {
  return [];
}

export async function deadJobs(): Promise<DeadJob[]> {
  return [];
}

export async function retryDead(): Promise<boolean> {
  return false;
}

export async function clearDead(): Promise<number> {
  return 0;
}
