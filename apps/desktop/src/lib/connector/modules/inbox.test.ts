import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { inboxModule } from './inbox';

/**
 * F-10b (оверлей-модуль): «Входящие» через `ctx` — оверлей + команда палитры. Стейт `inboxOpen`
 * остаётся ядром; `disposeAll` снимает вклады скопом.
 */

afterEach(() => modules._reset());

describe('inboxModule (F-10b)', () => {
  it('activate регистрирует оверлей и команду через ctx', () => {
    modules.register(inboxModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('inbox');
    expect(overlay?.titleKey).toBe('commands.view.inbox');
    expect(overlay?.order).toBe(50);
    expect(overlay?.isOpen({ inboxOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ inboxOpen: false } as UIState)).toBe(false);

    const cmd = commands.get('inbox:view.inbox');
    expect(cmd?.titleKey).toBe('commands.view.inbox');
    expect(cmd?.source).toBe('plugin');
    expect(commands.get('view.inbox')).toBeUndefined();
  });

  it('disposeAll снимает оверлей и команду скопом', () => {
    modules.register(inboxModule);
    modules.activateAll();
    expect(overlayRegistry.get('inbox')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('inbox')).toBeUndefined();
    expect(commands.get('inbox:view.inbox')).toBeUndefined();
  });
});
