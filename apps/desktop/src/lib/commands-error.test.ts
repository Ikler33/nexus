import { afterEach, describe, expect, it } from 'vitest';
import { commands } from './commands';
import { useToastStore } from '../stores/toast';

/**
 * F-8: CommandRegistry.run в try/catch — упавшая команда даёт ТОСТ, а не белый экран / висящий
 * reject (`commands.run` зовут через `void`). ErrorBoundary-эквивалент для команд.
 */

afterEach(() => {
  commands._reset();
  useToastStore.setState({ toasts: [] });
});

describe('commands.run — изоляция ошибок (F-8)', () => {
  it('падение команды → run резолвится (не бросает) + error-тост', async () => {
    commands.register({
      id: 'boom',
      title: 'Boom',
      titleKey: 'commands.view.home',
      run: () => {
        throw new Error('kaboom');
      },
    });

    await expect(commands.run('boom')).resolves.toBeUndefined();

    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0].kind).toBe('error');
    expect(toasts[0].message).toMatch(/не выполнена/i); // connector.commandFailed (ru)
  });

  it('успешная команда — тост не создаётся', async () => {
    let ran = false;
    commands.register({ id: 'ok', title: 'Ok', run: () => void (ran = true) });
    await commands.run('ok');
    expect(ran).toBe(true);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it('async-reject команды тоже ловится', async () => {
    commands.register({
      id: 'areject',
      title: 'AReject',
      run: () => Promise.reject(new Error('async fail')),
    });
    await expect(commands.run('areject')).resolves.toBeUndefined();
    expect(useToastStore.getState().toasts.some((t) => t.kind === 'error')).toBe(true);
  });
});
