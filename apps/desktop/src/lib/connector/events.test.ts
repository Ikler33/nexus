import { afterEach, describe, expect, it, vi } from 'vitest';
import { VAULT_SWITCHED_EVENT } from '../app-events';
import { tauriApi } from '../tauri-api';
import { onCoreEvent } from './events';

/**
 * Lifecycle-события коннектора (F-8): подписка/отписка через существующие каналы (НЕ новая шина).
 * `vault:opened` детерминирован в jsdom (window-событие F-1). `vault:changed`/`jobs:changed` вне
 * Tauri — best-effort no-op (bridge.subscribe), проверяем что делегируют в доменную подписку и
 * корректно снимаются.
 */

afterEach(() => vi.restoreAllMocks());

describe('onCoreEvent (F-8)', () => {
  it('vault:opened — эмит window-события `vault:switched` зовёт подписчика; dispose снимает', () => {
    const cb = vi.fn();
    const sub = onCoreEvent('vault:opened', cb);

    window.dispatchEvent(new Event(VAULT_SWITCHED_EVENT));
    expect(cb).toHaveBeenCalledTimes(1);

    window.dispatchEvent(new Event(VAULT_SWITCHED_EVENT));
    expect(cb).toHaveBeenCalledTimes(2);

    sub.dispose();
    window.dispatchEvent(new Event(VAULT_SWITCHED_EVENT));
    expect(cb).toHaveBeenCalledTimes(2); // после dispose — не зовётся
  });

  it('vault:changed делегирует в tauriApi.events.onVaultChanged', () => {
    const spy = vi
      .spyOn(tauriApi.events, 'onVaultChanged')
      .mockResolvedValue(() => {});
    const cb = vi.fn();
    const sub = onCoreEvent('vault:changed', cb);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toBe(cb);
    expect(() => sub.dispose()).not.toThrow();
  });

  it('jobs:changed делегирует в tauriApi.events.onJobsChanged', () => {
    const spy = vi.spyOn(tauriApi.events, 'onJobsChanged').mockResolvedValue(() => {});
    const cb = vi.fn();
    const sub = onCoreEvent('jobs:changed', cb);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(() => sub.dispose()).not.toThrow();
  });

  it('dispose ДО резолва async-подписки снимает её по резолву (не висит)', async () => {
    const unlisten = vi.fn();
    let resolveNow: (fn: () => void) => void = () => {};
    const pending = new Promise<() => void>((res) => {
      resolveNow = res;
    });
    vi.spyOn(tauriApi.events, 'onVaultChanged').mockReturnValue(pending);

    const sub = onCoreEvent('vault:changed', vi.fn());
    sub.dispose(); // раньше резолва
    resolveNow(unlisten);
    await pending;
    expect(unlisten).toHaveBeenCalledTimes(1); // подписка снята сразу по резолву
  });
});
