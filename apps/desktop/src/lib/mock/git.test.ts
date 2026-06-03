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
});
