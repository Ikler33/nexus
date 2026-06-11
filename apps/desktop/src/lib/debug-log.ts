import { invoke } from '@tauri-apps/api/core';

import { isTauri } from './tauri-api';

/**
 * Журнал действий UI (режим отладки, 2026-06-11): шлёт СОБЫТИЕ интерфейса в бэкенд-журнал
 * (stdout + файловый лог с ротацией). Ловит отчёты «кликнул — ничего не произошло»: по журналу
 * видно, дошло ли действие до обработчика и что случилось дальше.
 *
 * ПРИВАТНОСТЬ (принцип AC-SEC-6): передавайте только имена действий и метаданные (режим, длины,
 * коды) — НИКОГДА контент заметок/вопросов/статей. Best-effort: ошибки глотаются, вне Tauri — no-op.
 */
export function logUi(event: string, detail?: string): void {
  if (!isTauri()) return;
  void invoke('log_ui_event', { event, detail: detail ?? null }).catch(() => {});
}

/** Глобальные хуки ошибок фронта → журнал (однократная установка из main.tsx). */
export function installErrorLog(): void {
  if (!isTauri()) return;
  window.addEventListener('error', (e) => {
    const msg = String(e.error?.stack ?? e.message);
    // Браузерный шум, не ошибка приложения (Chrome/WebKit кидают при «опоздавших» нотификациях).
    if (msg.includes('ResizeObserver loop')) return;
    logUi('js-error', msg.slice(0, 600));
  });
  window.addEventListener('unhandledrejection', (e) => {
    logUi('js-unhandled-rejection', String(e.reason?.stack ?? e.reason).slice(0, 600));
  });
}
