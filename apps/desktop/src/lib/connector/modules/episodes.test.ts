import { afterEach, describe, expect, it, vi } from 'vitest';
import type { UIState } from '../../../stores/ui';
import { VAULT_SWITCHED_EVENT } from '../../app-events';
import { tauriApi } from '../../tauri-api';
import { usePrefsStore } from '../../../stores/prefs';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { episodesModule } from './episodes';

/**
 * Оверлей-модуль «Эпизоды»: оверлей (компонент + `isOpen`-селектор) + episodic-sync по `vault:opened`
 * (F-8b: перенос эффекта App.tsx). Стейт `episodesOpen` + open/close/toggle остаются ядром; `disposeAll`
 * снимает и оверлей, и подписку на событие скопом.
 */

afterEach(() => {
  modules._reset();
  vi.restoreAllMocks();
});

describe('episodesModule', () => {
  it('activate регистрирует оверлей (компонент + isOpen)', () => {
    modules.register(episodesModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('episodes');
    expect(overlay?.titleKey).toBe('episode.title');
    expect(overlay?.order).toBe(30);
    expect(overlay?.isOpen({ episodesOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ episodesOpen: false } as UIState)).toBe(false);
  });

  it('vault:opened синхронизирует pref aiEpisodicMemory от бэка (перенос episodic-sync App.tsx, F-8b)', async () => {
    const getEnabled = vi.spyOn(tauriApi.episode, 'getEnabled').mockResolvedValue(true);
    usePrefsStore.setState({ aiEpisodicMemory: false });
    modules.register(episodesModule);
    modules.activateAll();

    // `vault:opened` = window-событие `vault:switched` (см. connector/events.ts). Эмитим — эффект фетчит
    // persisted-флаг и отражает во фронт-pref.
    window.dispatchEvent(new Event(VAULT_SWITCHED_EVENT));
    await Promise.resolve();
    await Promise.resolve();

    expect(getEnabled).toHaveBeenCalledTimes(1);
    expect(usePrefsStore.getState().aiEpisodicMemory).toBe(true);
  });

  it('disposeAll снимает оверлей И подписку на vault:opened скопом', async () => {
    const getEnabled = vi.spyOn(tauriApi.episode, 'getEnabled').mockResolvedValue(true);
    modules.register(episodesModule);
    modules.activateAll();
    expect(overlayRegistry.get('episodes')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('episodes')).toBeUndefined();

    // После dispose подписка на vault:opened снята — повторный эмит НЕ ходит в бэк.
    window.dispatchEvent(new Event(VAULT_SWITCHED_EVENT));
    await Promise.resolve();
    expect(getEnabled).not.toHaveBeenCalled();
  });
});
