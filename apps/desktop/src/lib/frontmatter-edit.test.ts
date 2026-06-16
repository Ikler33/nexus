import { beforeEach, describe, expect, it, vi } from 'vitest';

import { FlushFailedError, writeFrontmatterField } from './frontmatter-edit';
import { tauriApi } from './tauri-api';
import { useWorkspaceStore } from '../stores/workspace';

describe('writeFrontmatterField (общий безопасный путь записи свойства)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    useWorkspaceStore.setState({ buffers: {} });
  });

  it('чистый буфер: пишет ключ + синхронизирует буфер (анти-эхо)', async () => {
    const setFm = vi
      .spyOn(tauriApi.vault, 'setFrontmatterField')
      .mockResolvedValue({ content: '---\nstatus: done\n---\n', hash: 'h2' });
    const sync = vi.spyOn(useWorkspaceStore.getState(), 'syncBufferAfterWrite');
    await writeFrontmatterField('t.md', 'status', 'done');
    expect(setFm).toHaveBeenCalledWith('t.md', 'status', 'done');
    expect(sync).toHaveBeenCalledWith('t.md', '---\nstatus: done\n---\n', 'h2');
  });

  it('грязный буфер: флашит сперва; флаш не удался → FlushFailedError, frontmatter НЕ тронут (R1)', async () => {
    useWorkspaceStore.setState({
      buffers: { 't.md': { path: 't.md', doc: 'мои правки', dirty: true, baseHash: 'h0' } },
    });
    vi.spyOn(useWorkspaceStore.getState(), 'saveBuffer').mockResolvedValue(undefined); // dirty не снят
    const setFm = vi.spyOn(tauriApi.vault, 'setFrontmatterField');
    await expect(writeFrontmatterField('t.md', 'status', 'done')).rejects.toBeInstanceOf(
      FlushFailedError,
    );
    expect(setFm).not.toHaveBeenCalled();
    expect(useWorkspaceStore.getState().buffers['t.md'].doc).toBe('мои правки');
  });
});
