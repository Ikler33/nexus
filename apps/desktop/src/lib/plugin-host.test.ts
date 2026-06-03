import { describe, expect, it, vi } from 'vitest';

import { commands } from './commands';
import { attachPlugin } from './plugin-host';
import { tauriApi } from './tauri-api';

/** Один RPC по порту: шлём запрос, ждём ответ с тем же `id` (или таймаут — чтобы тест не висел). */
function rpc(port: MessagePort, req: Record<string, unknown>): Promise<Record<string, unknown>> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve({ timeout: true }), 300);
    const handler = (e: MessageEvent) => {
      const data = e.data as { id?: unknown };
      if (data?.id !== req.id) return;
      clearTimeout(timer);
      port.removeEventListener('message', handler);
      resolve(e.data as Record<string, unknown>);
    };
    port.addEventListener('message', handler);
    port.start();
    port.postMessage(req);
  });
}

describe('plugin-host транспорт (attachPlugin)', () => {
  it('обслуживает vault.listFiles → ok с массивом записей', async () => {
    const ch = new MessageChannel();
    const handle = await attachPlugin('hello', ch.port1);
    const res = await rpc(ch.port2, { id: 1, method: 'vault.listFiles', path: '' });
    expect(res.ok).toBe(true);
    expect(Array.isArray(res.result)).toBe(true);
    handle.dispose();
  });

  it('читает файл в scope и пишет в Notes/, но отклоняет запись вне scope', async () => {
    const ch = new MessageChannel();
    const handle = await attachPlugin('hello', ch.port1);

    const read = await rpc(ch.port2, { id: 1, method: 'vault.readFile', path: 'README.md' });
    expect(read.ok).toBe(true);
    expect(typeof read.result).toBe('string');

    const wOk = await rpc(ch.port2, {
      id: 2,
      method: 'vault.writeFile',
      path: 'Notes/Idea.md',
      content: 'edited',
    });
    expect(wOk.ok).toBe(true);

    const wDenied = await rpc(ch.port2, {
      id: 3,
      method: 'vault.writeFile',
      path: 'README.md',
      content: 'hacked',
    });
    expect(wDenied.ok).toBe(false);
    expect(String(wDenied.error)).toContain('vault:write');
    handle.dispose();
  });

  it('confused-deputy: токен из payload ИГНОРИРУЕТСЯ — используется привязанный к порту', async () => {
    const spy = vi.spyOn(tauriApi.plugins, 'invoke');
    const ch = new MessageChannel();
    const handle = await attachPlugin('hello', ch.port1);

    // Плагин подсовывает чужой `token` в payload — релей обязан его игнорировать.
    const res = await rpc(ch.port2, {
      id: 1,
      method: 'vault.listFiles',
      path: '',
      token: 'evil-stolen-token',
    });
    expect(res.ok).toBe(true);

    const usedToken = spy.mock.calls[0]?.[0];
    expect(usedToken).toMatch(/^mock-tok-/); // токен сессии, host-side
    expect(usedToken).not.toBe('evil-stolen-token');
    spy.mockRestore();
    handle.dispose();
  });

  it('мусорное сообщение (без id/method) не вызывает ответа', async () => {
    const ch = new MessageChannel();
    const handle = await attachPlugin('hello', ch.port1);
    const res = await rpc(ch.port2, { id: 'not-a-number', foo: 1 });
    expect(res).toEqual({ timeout: true }); // ответа нет
    handle.dispose();
  });

  it('после dispose() вызовы не обслуживаются', async () => {
    const ch = new MessageChannel();
    const handle = await attachPlugin('hello', ch.port1);
    handle.dispose();
    const res = await rpc(ch.port2, { id: 1, method: 'vault.listFiles', path: '' });
    expect(res).toEqual({ timeout: true });
  });

  it('ui.registerCommand: команда в реестре, run шлёт событие плагину, dispose снимает', async () => {
    const ch = new MessageChannel();
    const handle = await attachPlugin('hello', ch.port1);

    const events: PluginEvent[] = [];
    ch.port2.addEventListener('message', (e: MessageEvent) => {
      const d = e.data as PluginEvent;
      if (d?.type === 'command') events.push(d);
    });
    ch.port2.start();

    const res = await rpc(ch.port2, {
      id: 1,
      method: 'ui.registerCommand',
      command: { id: 'sayhi', title: 'Hello: hi' },
    });
    expect(res.ok).toBe(true);

    const cmd = commands.get('plugin:hello:sayhi');
    expect(cmd?.source).toBe('plugin');
    expect(cmd?.title).toBe('Hello: hi');

    // запуск команды (как из палитры) → событие назад плагину
    await commands.run('plugin:hello:sayhi');
    await new Promise((r) => setTimeout(r, 20));
    expect(events).toContainEqual({ type: 'command', commandId: 'sayhi' });

    // dispose снимает команду из реестра
    handle.dispose();
    expect(commands.get('plugin:hello:sayhi')).toBeUndefined();
  });
});

interface PluginEvent {
  type: 'command';
  commandId: string;
}
