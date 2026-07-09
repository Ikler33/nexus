import { beforeEach, describe, expect, it } from 'vitest';

import * as plugins from './plugins';

describe('mock capability-брокер (превью)', () => {
  beforeEach(() => plugins.__resetForTests()); // изоляция: disable/remove не «протекают» между тестами


  it('list возвращает совместимый демо-плагин', async () => {
    const list = await plugins.list();
    expect(list.some((p) => p.dir === 'hello' && p.compatible)).toBe(true);
  });

  it('openSession неизвестного плагина → ошибка', async () => {
    await expect(plugins.openSession('nope')).rejects.toThrow();
  });

  it('invoke с неизвестным токеном → ошибка (сессия не найдена)', async () => {
    await expect(plugins.invoke('bogus', 'vault.listFiles', '')).rejects.toThrow();
  });

  it('scope: читает любой файл (read **), пишет в Notes/** (вкл. вложенные), вне — отказ', async () => {
    const token = await plugins.openSession('hello');
    await expect(plugins.invoke(token, 'vault.readFile', 'Projects/Roadmap.md')).resolves.toBeTypeOf(
      'string',
    );
    await expect(
      plugins.invoke(token, 'vault.writeFile', 'Notes/Idea.md', 'x'),
    ).resolves.toMatchObject({ ok: true });
    await expect(
      plugins.invoke(token, 'vault.writeFile', 'Notes/sub/deep.md', 'x'),
    ).resolves.toMatchObject({ ok: true });
    await expect(plugins.invoke(token, 'vault.writeFile', 'README.md', 'x')).rejects.toThrow(
      /vault:write/,
    );
  });

  it('неизвестный метод → ошибка', async () => {
    const token = await plugins.openSession('hello');
    await expect(plugins.invoke(token, 'vault.nuke', 'x')).rejects.toThrow();
  });

  it('closeSession отзывает сессию: последующий invoke падает', async () => {
    const token = await plugins.openSession('hello');
    await expect(plugins.invoke(token, 'vault.listFiles', '')).resolves.toBeDefined();
    await plugins.closeSession(token);
    await expect(plugins.invoke(token, 'vault.listFiles', '')).rejects.toThrow();
  });

  it('ai: embed → вектор, searchSemantic → выдача (право ai:embed)', async () => {
    const token = await plugins.openSession('hello');
    await expect(plugins.invoke(token, 'ai.embed', undefined, 'hi')).resolves.toHaveLength(16);
    const hits = await plugins.invoke(token, 'ai.searchSemantic', undefined, 'roadmap');
    expect(Array.isArray(hits)).toBe(true);
  });

  it('net.fetch: allowlisted host → ok, прочий → отказ', async () => {
    const token = await plugins.openSession('hello');
    await expect(
      plugins.invoke(token, 'net.fetch', 'https://api.github.com/repos/x'),
    ).resolves.toMatchObject({ status: 200 });
    await expect(plugins.invoke(token, 'net.fetch', 'https://evil.example.com/x')).rejects.toThrow(
      /allowlist/,
    );
  });

  // ── Durable-журнал доступа (PLUG-1, зеркало backend plugin_audit / list_plugin_audit) ──

  it('auditLog: каждый invoke append-only-записывается (allow+deny), свежие первыми', async () => {
    const token = await plugins.openSession('hello');
    // allow (read **), allow (write Notes/**), deny (write вне scope).
    await plugins.invoke(token, 'vault.readFile', 'Projects/Roadmap.md');
    await plugins.invoke(token, 'vault.writeFile', 'Notes/Idea.md', 'x');
    await expect(
      plugins.invoke(token, 'vault.writeFile', 'README.md', 'x'),
    ).rejects.toThrow();

    const log = await plugins.auditLog(100);
    expect(log).toHaveLength(3);
    // Свежие первыми (как ORDER BY id DESC): последний вызов (deny на README.md) — первым.
    expect(log[0]).toMatchObject({
      method: 'vault.writeFile',
      target: 'README.md',
      allowed: false,
      pluginId: 'hello-reader',
    });
    expect(log[0].deniedReason).toBeTruthy();
    // Старейший (read Roadmap) — последним, allowed, без причины отказа.
    expect(log[2]).toMatchObject({
      method: 'vault.readFile',
      target: 'Projects/Roadmap.md',
      allowed: true,
      deniedReason: null,
    });
    // Монотонность id (append-only): свежая запись — больший id.
    expect(log[0].id).toBeGreaterThan(log[2].id);
  });

  it('auditLog: limit зажимается и отдаёт только последние N', async () => {
    const token = await plugins.openSession('hello');
    await plugins.invoke(token, 'vault.listFiles', '');
    await plugins.invoke(token, 'vault.listFiles', '');
    await plugins.invoke(token, 'vault.listFiles', '');
    const log = await plugins.auditLog(2);
    expect(log).toHaveLength(2); // только 2 свежайших
  });

  // ── Управление (enable/disable/remove) — зеркало backend-контракта ──

  it('setEnabled выключает: list.enabled=false, openSession отказывает; включение восстанавливает', async () => {
    await plugins.setEnabled('hello', false);
    const off = await plugins.list();
    expect(off.find((p) => p.dir === 'hello')?.enabled).toBe(false);
    await expect(plugins.openSession('hello')).rejects.toThrow(/выключен/);

    await plugins.setEnabled('hello', true); // восстановить для остальных тестов
    const on = await plugins.list();
    expect(on.find((p) => p.dir === 'hello')?.enabled).toBe(true);
    await expect(plugins.openSession('hello')).resolves.toMatch(/^mock-tok-/);
  });

  // ПОСЛЕДНИЙ тест: remove «sticky» в рамках модуля (нет un-remove) — ставим в конце, чтобы не влиять.
  it('remove убирает плагин из list и блокирует openSession', async () => {
    await plugins.remove('hello');
    const list = await plugins.list();
    expect(list.some((p) => p.dir === 'hello')).toBe(false);
    await expect(plugins.openSession('hello')).rejects.toThrow();
  });
});
