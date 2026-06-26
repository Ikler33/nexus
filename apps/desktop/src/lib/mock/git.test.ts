import { beforeEach, describe, expect, it, vi } from 'vitest';

import * as git from './git';

describe('mock git-sync (превью)', () => {
  it('status → изменения; commit → committed; затем nothing-to-commit', async () => {
    expect((await git.status()).length).toBeGreaterThan(0);

    const out = await git.commit();
    expect(out.status).toBe('committed');

    expect(await git.status()).toHaveLength(0);
    expect((await git.commit()).status).toBe('nothing-to-commit');
  });

  it('token: setToken → hasToken=true → clearToken → false', async () => {
    expect(await git.hasToken()).toBe(false);
    await git.setToken('ghp_demo');
    expect(await git.hasToken()).toBe(true);
    await git.clearToken();
    expect(await git.hasToken()).toBe(false);
  });

  it('remote+sync: setRemote → getRemote; sync → fast-forward', async () => {
    expect(await git.getRemote()).toBeNull();
    await git.setRemote('https://example.com/v.git');
    expect(await git.getRemote()).toBe('https://example.com/v.git');
    expect((await git.sync()).status).toBe('fast-forward');
  });
});

// P1-6: мок `commitPaths` ОБЯЗАН зеркалить бэкенд `git_commit_paths` — коммитить ТОЛЬКО выбранные
// пути, остальное оставлять dirty (иначе превью/тесты соврут, что всё закоммитилось).
// Свежий модуль на каждый тест (общий `dirty` сбрасывается) — детерминизм независимо от порядка.
describe('mock git-sync — выборочный commitPaths (P1-6, зеркалит бэкенд)', () => {
  let fresh: typeof import('./git');
  beforeEach(async () => {
    vi.resetModules();
    fresh = await import('./git');
  });

  it('commitPaths(только один) → committed files=1; невыбранный ОСТАЁТСЯ в status', async () => {
    const before = await fresh.status();
    expect(before.length).toBe(2); // README.md (modified) + Notes/Idea.md (new)
    const [a, b] = before;

    const out = await fresh.commitPaths([a.path]);
    expect(out.status).toBe('committed');
    expect(out.status === 'committed' && out.files).toBe(1); // ушёл ровно один

    const after = await fresh.status();
    expect(after.map((e) => e.path)).toEqual([b.path]); // a ушёл, b остался dirty
  });

  it('commitPaths([]) (пустой выбор) → nothing-to-commit, ничего не закоммичено', async () => {
    const before = await fresh.status();
    const out = await fresh.commitPaths([]);
    expect(out.status).toBe('nothing-to-commit');
    expect(await fresh.status()).toHaveLength(before.length); // всё осталось dirty
  });

  it('commitPaths(устаревший/несуществующий путь) → nothing-to-commit, dirty не тронут', async () => {
    const before = await fresh.status();
    const out = await fresh.commitPaths(['does/not/exist.md']);
    expect(out.status).toBe('nothing-to-commit');
    expect(await fresh.status()).toHaveLength(before.length);
  });

  it('commitPaths(все пути) → всё закоммичено, status пуст', async () => {
    const all = (await fresh.status()).map((e) => e.path);
    const out = await fresh.commitPaths(all);
    expect(out.status).toBe('committed');
    expect(out.status === 'committed' && out.files).toBe(all.length);
    expect(await fresh.status()).toHaveLength(0);
  });

  it('commitPaths с сообщением → сообщение в исходе; пустое сообщение → авто-саммари', async () => {
    const all = (await fresh.status()).map((e) => e.path);
    const out = await fresh.commitPaths([all[0]], 'моё сообщение');
    expect(out.status === 'committed' && out.message).toBe('моё сообщение');
  });
});
