import { afterEach, describe, expect, it } from 'vitest';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { episodesModule } from './episodes';

/**
 * F-10b (оверлей-модуль): «Эпизоды» — самый узкий вклад (ТОЛЬКО оверлей, ни команды, ни события).
 * Стейт `episodesOpen` + open/close/toggle остаются ядром; `disposeAll` снимает оверлей.
 */

afterEach(() => modules._reset());

describe('episodesModule (F-10b)', () => {
  it('activate регистрирует ТОЛЬКО оверлей (компонент + isOpen)', () => {
    modules.register(episodesModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('episodes');
    expect(overlay?.titleKey).toBe('episode.title');
    expect(overlay?.order).toBe(30);
    expect(overlay?.isOpen({ episodesOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ episodesOpen: false } as UIState)).toBe(false);
  });

  it('disposeAll снимает оверлей', () => {
    modules.register(episodesModule);
    modules.activateAll();
    expect(overlayRegistry.get('episodes')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('episodes')).toBeUndefined();
  });
});
