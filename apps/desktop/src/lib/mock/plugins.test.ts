import { describe, expect, it } from 'vitest';

import * as plugins from './plugins';

describe('mock capability-брокер (превью)', () => {
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
});
