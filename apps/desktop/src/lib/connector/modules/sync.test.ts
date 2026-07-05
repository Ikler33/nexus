import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import type { UIState } from '../../../stores/ui';
import { modules } from '../module-manager';
import { overlayRegistry } from '../registries';
import { syncModule } from './sync';

/**
 * F-10c (оверлей-модуль): «Синхронизация» подключается в ядро ТОЛЬКО через `ctx` — модуль регистрирует
 * оверлей SyncPanel (компонент + `isOpen`-селектор) и команду палитры, а `disposeAll` снимает их скопом.
 * Стейт видимости `syncOpen` остаётся ядром (ui-стор) — модуль лишь читает его селектором.
 */

afterEach(() => modules._reset());

describe('syncModule (F-10c)', () => {
  it('activate регистрирует оверлей (order/titleKey/isOpen) и команду палитры через ctx', () => {
    modules.register(syncModule);
    modules.activateAll();

    const overlay = overlayRegistry.get('sync');
    expect(overlay?.titleKey).toBe('commands.view.sync');
    expect(overlay?.order).toBe(80);
    // isOpen — селектор поверх ядрового флага `syncOpen`.
    expect(overlay?.isOpen({ syncOpen: true } as UIState)).toBe(true);
    expect(overlay?.isOpen({ syncOpen: false } as UIState)).toBe(false);

    // Команда палитры — id префиксован модулем (`sync:view.sync`), source=plugin.
    const cmd = commands.get('sync:view.sync');
    expect(cmd?.titleKey).toBe('commands.view.sync');
    expect(cmd?.source).toBe('plugin');
    // Непрефиксованного id `view.sync` в реестре больше НЕТ (ядро его не регистрирует).
    expect(commands.get('view.sync')).toBeUndefined();
  });

  it('disposeAll снимает оверлей и команду скопом', () => {
    modules.register(syncModule);
    modules.activateAll();
    expect(overlayRegistry.get('sync')).toBeDefined();

    modules.disposeAll();
    expect(overlayRegistry.get('sync')).toBeUndefined();
    expect(commands.get('sync:view.sync')).toBeUndefined();
  });
});
