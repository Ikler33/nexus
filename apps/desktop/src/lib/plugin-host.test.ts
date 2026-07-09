import { describe, expect, it, vi } from 'vitest';

import i18n from '../i18n/setup';
import { commands } from './commands';
import { PLUGIN_CSP, attachPlugin, demoPluginSrcdoc, withPluginCsp } from './plugin-host';
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

  it('ui.addTranslations + titleKey: заголовок команды локализуется (namespace plugin:<id>)', async () => {
    const ch = new MessageChannel();
    const handle = await attachPlugin('hello', ch.port1);

    const tr = await rpc(ch.port2, {
      id: 1,
      method: 'ui.addTranslations',
      translations: { ru: { greetKey: 'Привет' }, en: { greetKey: 'Hi' } },
    });
    expect(tr.ok).toBe(true);
    expect(i18n.t('plugin:hello:greetKey', { lng: 'ru' })).toBe('Привет');
    expect(i18n.t('plugin:hello:greetKey', { lng: 'en' })).toBe('Hi');

    const reg = await rpc(ch.port2, {
      id: 2,
      method: 'ui.registerCommand',
      command: { id: 'greet', title: 'fallback', titleKey: 'greetKey' },
    });
    expect(reg.ok).toBe(true);
    expect(commands.get('plugin:hello:greet')?.titleKey).toBe('plugin:hello:greetKey');

    handle.dispose();
  });
});

/**
 * Egress-контейнмент плагинного iframe (THREAT_MODEL T2). JSDOM НЕ энфорсит CSP, поэтому здесь —
 * регресс-пин: srcdoc ОБЯЗАН нести точную жёсткую CSP первым meta в <head> (иначе `fetch`/`img`/
 * `sendBeacon` на внешний хост из iframe открыты). Live-энфорсмент проверяется в реальном Tauri-app.
 */
describe('plugin CSP egress-контейнмент (T2)', () => {
  it('PLUGIN_CSP: connect/img/media/font/form/frame/base = none; script+style unsafe-inline', () => {
    expect(PLUGIN_CSP).toBe(
      "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; " +
        "connect-src 'none'; img-src 'none'; media-src 'none'; font-src 'none'; " +
        "form-action 'none'; frame-src 'none'; base-uri 'none'",
    );
  });

  it('withPluginCsp: CSP-meta вставлен ПЕРВЫМ тегом в <head>', () => {
    const out = withPluginCsp('<!doctype html><html><head><meta charset="utf-8"></head><body>x</body></html>');
    const expectedMeta = `<meta http-equiv="Content-Security-Policy" content="${PLUGIN_CSP}">`;
    expect(out).toContain(expectedMeta);
    // Именно ПЕРВЫМ после <head>: браузер применяет CSP к тому, что объявлено ПОСЛЕ meta.
    expect(out.indexOf('<head>') + '<head>'.length).toBe(out.indexOf(expectedMeta));
  });

  it('withPluginCsp: fail-closed при отсутствии <head>', () => {
    expect(() => withPluginCsp('<!doctype html><html><body>no head</body></html>')).toThrow(/head/);
  });

  it('demoPluginSrcdoc: несёт точную CSP первым meta в <head> (регресс-пин)', () => {
    const html = demoPluginSrcdoc();
    const expectedMeta = `<meta http-equiv="Content-Security-Policy" content="${PLUGIN_CSP}">`;
    expect(html).toContain(expectedMeta);
    expect(html.indexOf('<head>') + '<head>'.length).toBe(html.indexOf(expectedMeta));
    // connect-src 'none' — суть контейнмента egress: fetch/XHR/beacon наружу заблокированы.
    expect(html).toContain("connect-src 'none'");
    // Behavior-preserving: демо НЕ тянет внешних ресурсов (иначе CSP их бы срезала).
    expect(html).not.toMatch(/https?:\/\//); // нет внешних URL в демо
    expect(html).not.toContain('fetch('); // демо ходит к брокеру через postMessage, не fetch
  });
});

interface PluginEvent {
  type: 'command';
  commandId: string;
}
