import { afterEach, describe, expect, it } from 'vitest';
import { commands } from '../../commands';
import { modules } from '../module-manager';
import { settingsRegistry, viewRegistry } from '../registries';
import { boardModule } from './board';

/**
 * F-10c (вью-модуль): «Доска» подключается в ядро ТОЛЬКО через `ctx` — модуль регистрирует main-вью,
 * а `disposeAll` снимает её. В отличие от news-эталона у board НЕТ секции настроек и НЕТ команды
 * палитры (ядро никогда не объявляло `view.board`). Стейт `mainView` остаётся ядром (ui-стор).
 */

afterEach(() => modules._reset());

describe('boardModule (F-10c)', () => {
  it('activate регистрирует main-вью (order/titleKey/activityBar) через ctx', () => {
    modules.register(boardModule);
    modules.activateAll();

    // Main-вью «Доска» — с прежними order/titleKey/activityBar (behavior-preserving).
    const view = viewRegistry.get('board');
    expect(view?.titleKey).toBe('commands.view.board');
    expect(view?.order).toBe(40);
    expect(view?.activityBar).toBe(true);
    expect(view?.isActive('board')).toBe(true);
    expect(view?.isActive('news')).toBe(false);

    // Board НЕ даёт ни секции настроек, ни команды палитры (behavior-preserving: их не было).
    expect(settingsRegistry.list().some((s) => s.id === 'board')).toBe(false);
    expect(commands.get('board:view.board')).toBeUndefined();
    expect(commands.get('view.board')).toBeUndefined();
  });

  it('disposeAll снимает main-вью board', () => {
    modules.register(boardModule);
    modules.activateAll();
    expect(viewRegistry.get('board')).toBeDefined();

    modules.disposeAll();
    expect(viewRegistry.get('board')).toBeUndefined();
  });
});
