import { afterEach, describe, expect, it, vi } from 'vitest';
import { commands } from '../commands';
import { modules } from './module-manager';
import { viewRegistry } from './registries';
import type { NexusModule } from './types';

/**
 * Менеджер модулей коннектора (F-8): детерминированный порядок активации, префикс команд
 * `${moduleId}:`, скоупированный dispose (снятие всех вкладов модуля скопом), идемпотентная
 * регистрация. Реальных модулей в проде ноль — здесь тест-заглушки.
 */

afterEach(() => modules._reset());

const nullComp = () => null;

describe('ModuleManager (F-8)', () => {
  it('активирует модули в порядке регистрации (детерминированно)', () => {
    const order: string[] = [];
    const mk = (id: string): NexusModule => ({ id, activate: () => order.push(id) });
    modules.register(mk('a'));
    modules.register(mk('b'));
    modules.register(mk('c'));
    modules.activateAll();
    expect(order).toEqual(['a', 'b', 'c']);
  });

  it('дубликат id при register — no-op (идемпотентность)', () => {
    modules.register({ id: 'dup', activate: () => {} });
    modules.register({ id: 'dup', activate: () => {} });
    expect(modules.list()).toHaveLength(1);
  });

  it('ctx.commands.register префиксует id `${moduleId}:` и ставит source=plugin', () => {
    modules.register({
      id: 'mymod',
      activate: (ctx) => ctx.commands.register({ id: 'ping', title: 'Ping', run: () => {} }),
    });
    modules.activateAll();
    const cmd = commands.get('mymod:ping');
    expect(cmd).toBeDefined();
    expect(cmd?.source).toBe('plugin');
    expect(commands.get('ping')).toBeUndefined(); // без префикса не регистрируется
  });

  it('всё зарегистрированное копится в ctx.subscriptions', () => {
    let count = -1;
    modules.register({
      id: 'subs',
      activate: (ctx) => {
        ctx.commands.register({ id: 'c1', title: 'c1', run: () => {} });
        ctx.views.register({
          id: 'subs:v',
          titleKey: 't',
          icon: nullComp,
          order: 1,
          component: nullComp,
          activate: () => {},
          isActive: () => false,
        });
        ctx.events.on('vault:opened', () => {});
        count = ctx.subscriptions.length;
      },
    });
    modules.activateAll();
    expect(count).toBe(3);
  });

  it('disposeAll снимает вклады модуля скопом (команды + вью)', () => {
    modules.register({
      id: 'scoped',
      activate: (ctx) => {
        ctx.commands.register({ id: 'x', title: 'x', run: () => {} });
        ctx.views.register({
          id: 'scoped:view',
          titleKey: 't',
          icon: nullComp,
          order: 900,
          component: nullComp,
          activate: () => {},
          isActive: () => false,
        });
      },
    });
    modules.activateAll();
    expect(commands.get('scoped:x')).toBeDefined();
    expect(viewRegistry.get('scoped:view')).toBeDefined();

    modules.disposeAll();
    expect(commands.get('scoped:x')).toBeUndefined();
    expect(viewRegistry.get('scoped:view')).toBeUndefined();
  });

  it('events-подписка модуля снимается при dispose (unlisten вызван)', () => {
    // vault:opened — window-подписка; после dispose window-событие подписчика не зовёт.
    const cb = vi.fn();
    modules.register({ id: 'ev', activate: (ctx) => ctx.events.on('vault:opened', cb) });
    modules.activateAll();
    window.dispatchEvent(new Event('vault:switched'));
    expect(cb).toHaveBeenCalledTimes(1);

    modules.disposeAll();
    window.dispatchEvent(new Event('vault:switched'));
    expect(cb).toHaveBeenCalledTimes(1); // снят
  });
});
