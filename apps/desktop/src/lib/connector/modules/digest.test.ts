import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { digestModule } from './digest';

/**
 * F-10b (оверлей-модуль): «Дайджест изменений» через `ctx` — float-оверлей + команда + refetch по
 * `jobs:changed`. Стейт `digestOpen` остаётся ядром; `disposeAll` снимает вклады скопом.
 */

afterEach(() => modules._reset());

describe('digestModule (F-10b)', () => {
  it('activate регистрирует оверлей и команду через ctx', () => {
    modules.register(digestModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('digest');
    expect(overlay?.titleKey).toBe('commands.view.digest');
    expect(overlay?.order).toBe(60);
    expect(overlay?.isOpen({ digestOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ digestOpen: false } as UIState)).toBe(false);

    const cmd = commands.get('digest:view.digest');
    expect(cmd?.titleKey).toBe('commands.view.digest');
    expect(cmd?.source).toBe('plugin');
    expect(commands.get('view.digest')).toBeUndefined();
  });

  it('disposeAll снимает оверлей и команду скопом', () => {
    modules.register(digestModule);
    modules.activateAll();
    expect(overlayRegistry.get('digest')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('digest')).toBeUndefined();
    expect(commands.get('digest:view.digest')).toBeUndefined();
  });
});
