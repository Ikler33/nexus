import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { memoryModule } from './memory';

/**
 * F-10b (оверлей-модуль): «Память ИИ» через `ctx` — оверлей (компонент + `isOpen`) + команда палитры.
 * Стейт видимости `memoryOpen` остаётся ядром (ui-стор); `disposeAll` снимает вклады скопом.
 */

afterEach(() => modules._reset());

describe('memoryModule (F-10b)', () => {
  it('activate регистрирует оверлей и команду через ctx', () => {
    modules.register(memoryModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('memory');
    expect(overlay?.titleKey).toBe('commands.view.memory');
    expect(overlay?.order).toBe(20);
    expect(overlay?.isOpen({ memoryOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ memoryOpen: false } as UIState)).toBe(false);

    const cmd = commands.get('memory:view.memory');
    expect(cmd?.titleKey).toBe('commands.view.memory');
    expect(cmd?.source).toBe('plugin');
    expect(commands.get('view.memory')).toBeUndefined();
  });

  it('disposeAll снимает оверлей и команду скопом', () => {
    modules.register(memoryModule);
    modules.activateAll();
    expect(overlayRegistry.get('memory')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('memory')).toBeUndefined();
    expect(commands.get('memory:view.memory')).toBeUndefined();
  });
});
