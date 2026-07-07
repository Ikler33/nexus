import { invoke, type InvokeArgs } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/**
 * F-2a: инфраструктура моста фронт ↔ нативный слой.
 *
 * Единственная точка, где живёт развилка «Tauri-webview vs браузер/vitest»: в Tauri команда
 * уходит в Rust через `invoke`, вне Tauri — прозрачно проксируется в мок-бэкенд (`lib/mock/*`,
 * DESIGN §0). Доменные модули `lib/api/<домен>/` строят свои вызовы ТОЛЬКО через `bridge`/
 * `subscribe` — инлайн-моки в доменном коде запрещены (ratchet: `mock/parity.test.ts`, гейт (в)).
 *
 * Прямой Tauri IPC (`invoke`/`Channel`/`listen`) разрешён только в слое `lib/api/*` и — до конца
 * распила F-2 — в барреле `lib/tauri-api.ts` (контракт §4.1 ARCHITECTURE).
 *
 * ЧЕСТНЫЕ ИСКЛЮЧЕНИЯ (в bridge не влезают, остаются прямыми у себя в домене):
 * - Стрим-команды с `Channel` (`chat_rag`/`inline_complete`/`agent_run`): создают канал,
 *   подвешивают `onmessage`, возвращают функцию отмены — это не request/response-форма
 *   `bridge`, а другой контракт (события по ходу + отдельная cancel-команда).
 * - Пути с OS-диалогами (`backup.exportToFile`/`news.exportLogs`/`vault.pickDirectory` и т.п.):
 *   сначала диалог `@tauri-apps/plugin-dialog`, потом (иногда) `invoke` — развилка не сводится
 *   к паре «команда/мок».
 */

/** Запущены ли мы внутри Tauri-webview (а не в обычном браузере / тесте). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * Мост request/response-команды: в Tauri — `invoke<T>(cmd, args)`, вне Tauri — `mock()`.
 * `mock` обязан жить в `lib/mock/*` и зеркалить контракт Rust-команды (mock-must-match-backend).
 * Для команд без аргументов передавай `undefined` вторым параметром — явно, без магии.
 */
export function bridge<T>(
  cmd: string,
  args: InvokeArgs | undefined,
  mock: () => Promise<T>,
): Promise<T> {
  return isTauri() ? invoke<T>(cmd, args) : mock();
}

// ── Мок-шина событий (вне Tauri): позволяет мок-бэкенду (`lib/mock/*`) эмитить те же события,
// что нативный слой (`news:progress`, `jobs:changed`), в браузер-превью и vitest — иначе живые
// сигналы прогона были бы мёртвым кодом, а зелёные тесты врали (mock-must-match-backend / MEM-5).
// Пусто по умолчанию (никто не эмитит → поведение как прежний no-op). ─────────────────────────────
const mockSubscribers = new Map<string, Set<(payload: unknown) => void>>();

/**
 * Мост событийной подписки: в Tauri — `listen(event)` с колбэком на payload, вне Tauri —
 * регистрация в мок-шине (мок-бэкенд может эмитить через `mockEmit`). Возвращает функцию отписки.
 */
export async function subscribe<P>(
  event: string,
  cb: (payload: P) => void,
): Promise<() => void> {
  if (!isTauri()) {
    const cbAny = cb as (payload: unknown) => void;
    let set = mockSubscribers.get(event);
    if (!set) {
      set = new Set();
      mockSubscribers.set(event, set);
    }
    set.add(cbAny);
    return () => {
      mockSubscribers.get(event)?.delete(cbAny);
    };
  }
  return listen<P>(event, (e) => cb(e.payload));
}

/**
 * Эмит события в мок-шину (только вне Tauri; зовут мок-бэкенды `lib/mock/*`). Зеркалит `app.emit`
 * нативного слоя: доставляет `payload` всем подписчикам события. В Tauri — no-op (события шлёт Rust).
 */
export function mockEmit(event: string, payload?: unknown): void {
  if (isTauri()) return;
  mockSubscribers.get(event)?.forEach((cb) => cb(payload));
}
