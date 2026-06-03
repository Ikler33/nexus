import { describe, expect, it } from 'vitest';

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
