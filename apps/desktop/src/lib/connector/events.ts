/**
 * Реестр lifecycle-событий коннектора (F-8): тонкая обёртка над СУЩЕСТВУЮЩИМИ сигналами ядра — НЕ
 * новая шина (легализация паттернов F-1/F-2):
 *  - `vault:opened`  → window-событие `vault:switched` (F-1: эмитит `stores/vault.openVault`);
 *  - `vault:changed` → доменная подписка `tauriApi.events.onVaultChanged` (F-2a watcher);
 *  - `jobs:changed`  → доменная подписка `tauriApi.events.onJobsChanged` (F-2d scheduler).
 *
 * Возвращает `Disposable` с единой семантикой снятия (window `removeEventListener` / доменный
 * `unlisten`), в т.ч. корректно снимает подписку, если dispose пришёл до резолва async-подписки.
 */
import { VAULT_SWITCHED_EVENT } from '../app-events';
import { tauriApi } from '../tauri-api';
import type { CoreEvent, Disposable } from './types';

export function onCoreEvent(event: CoreEvent, cb: () => void): Disposable {
  if (event === 'vault:opened') {
    // F-1 window-событие: эмитент диспатчит на `window`, подписчик слушает сам (без прямых импортов).
    const handler = (): void => cb();
    window.addEventListener(VAULT_SWITCHED_EVENT, handler);
    return { dispose: () => window.removeEventListener(VAULT_SWITCHED_EVENT, handler) };
  }

  // F-2 доменные подписки — асинхронный резолв `unlisten`. Если dispose пришёл раньше резолва —
  // снимаем сразу по резолву (не оставляем висящую подписку).
  let unlisten = (): void => {};
  let disposed = false;
  const pending =
    event === 'vault:changed'
      ? tauriApi.events.onVaultChanged(cb)
      : tauriApi.events.onJobsChanged(cb);
  void pending.then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });
  return {
    dispose: () => {
      disposed = true;
      unlisten();
    },
  };
}
