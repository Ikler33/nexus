import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { contradictionsModule } from './contradictions';

/**
 * F-10b (оверлей-модуль): «Поиск противоречий» через `ctx` — float-оверлей + команда + refetch по
 * `jobs:changed`. Стейт `contradictionsOpen` остаётся ядром; `disposeAll` снимает вклады скопом.
 */

afterEach(() => modules._reset());

describe('contradictionsModule (F-10b)', () => {
  it('activate регистрирует оверлей и команду через ctx', () => {
    modules.register(contradictionsModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('contradictions');
    expect(overlay?.titleKey).toBe('commands.view.contradictions');
    expect(overlay?.order).toBe(70);
    expect(overlay?.isOpen({ contradictionsOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ contradictionsOpen: false } as UIState)).toBe(false);

    const cmd = commands.get('contradictions:view.contradictions');
    expect(cmd?.titleKey).toBe('commands.view.contradictions');
    expect(cmd?.source).toBe('plugin');
    expect(commands.get('view.contradictions')).toBeUndefined();
  });

  it('disposeAll снимает оверлей и команду скопом', () => {
    modules.register(contradictionsModule);
    modules.activateAll();
    expect(overlayRegistry.get('contradictions')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('contradictions')).toBeUndefined();
    expect(commands.get('contradictions:view.contradictions')).toBeUndefined();
  });
});
