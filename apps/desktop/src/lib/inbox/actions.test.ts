import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { INBOX } from '../daily';
import { tauriApi } from '../tauri-api';
import { useVaultStore } from '../../stores/vault';
import { type Buffer, useWorkspaceStore } from '../../stores/workspace';
import { discard, toNote, toTask } from './actions';

const INBOX_DOC = '# Inbox\n- 09:00 a\n- 10:00 b';

function setBuffers(specs: { path: string; doc: string; dirty: boolean }[]): void {
  const buffers: Record<string, Buffer> = {};
  for (const s of specs) buffers[s.path] = { path: s.path, doc: s.doc, dirty: s.dirty, baseHash: '' };
  useWorkspaceStore.setState({ buffers });
}

beforeEach(() => setBuffers([]));
afterEach(() => {
  vi.restoreAllMocks();
  setBuffers([]);
});

describe('inbox actions (INBOX-1)', () => {
  it('discard: открытый буфер Inbox → updateBufferDoc без строки', async () => {
    setBuffers([{ path: INBOX, doc: INBOX_DOC, dirty: false }]);
    expect(await discard({ line: 2, time: '09:00', text: 'a' })).toBe(true);
    expect(useWorkspaceStore.getState().buffers[INBOX].doc).toBe('# Inbox\n- 10:00 b');
  });

  it('discard: закрытый Inbox → read+write диска (строка вырезана)', async () => {
    const read = vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue(INBOX_DOC);
    const write = vi.spyOn(tauriApi.vault, 'writeFile').mockResolvedValue('h');
    expect(await discard({ line: 3, time: '10:00', text: 'b' })).toBe(true);
    expect(read).toHaveBeenCalledWith(INBOX);
    expect(write).toHaveBeenCalledWith(INBOX, '# Inbox\n- 09:00 a', false);
  });

  it('drift: текст строки не совпал → false, без записи', async () => {
    const write = vi.spyOn(tauriApi.vault, 'writeFile').mockResolvedValue('h');
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue(INBOX_DOC);
    expect(await discard({ line: 2, time: '09:00', text: 'другой текст' })).toBe(false);
    expect(write).not.toHaveBeenCalled();
  });

  it('сдвиг номеров: вторая операция со СТАРЫМ item.line НЕ no-op (матч по time+text)', async () => {
    // Панель загрузила оба элемента с исходными line (2 и 3). Триажим первый…
    setBuffers([{ path: INBOX, doc: INBOX_DOC, dirty: false }]);
    expect(await discard({ line: 2, time: '09:00', text: 'a' })).toBe(true);
    expect(useWorkspaceStore.getState().buffers[INBOX].doc).toBe('# Inbox\n- 10:00 b'); // b уехал на строку 2
    // …второй элемент держит УСТАРЕВШИЙ line:3, но матчится по time+text и режет фактическую строку.
    expect(await discard({ line: 3, time: '10:00', text: 'b' })).toBe(true);
    expect(useWorkspaceStore.getState().buffers[INBOX].doc).toBe('# Inbox');
  });

  it('toTask: дозаписывает `- [ ] текст` в дневник и убирает из Inbox', async () => {
    vi.spyOn(tauriApi.vault, 'fileHash').mockResolvedValue('h'); // дневник существует
    vi.spyOn(tauriApi.vault, 'readFile').mockImplementation((p: string) =>
      Promise.resolve(p === INBOX ? INBOX_DOC : '# daily\n'),
    );
    const write = vi.spyOn(tauriApi.vault, 'writeFile').mockResolvedValue('h');
    expect(await toTask({ line: 2, time: '09:00', text: 'a' })).toBe(true);
    const dailyCall = write.mock.calls.find((c) => String(c[0]).startsWith('Journal/'));
    expect(dailyCall?.[1]).toContain('- [ ] a');
    const inboxCall = write.mock.calls.find((c) => c[0] === INBOX);
    expect(inboxCall?.[1]).toBe('# Inbox\n- 10:00 b');
  });

  it('toNote: создаёт заметку из текста, открывает её и убирает из Inbox', async () => {
    const doc = '# Inbox\n- 09:00 идея проекта\n- 10:00 b';
    vi.spyOn(tauriApi.vault, 'fileHash').mockResolvedValue(null); // заметки ещё нет
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue(doc);
    const write = vi.spyOn(tauriApi.vault, 'writeFile').mockResolvedValue('h');
    vi.spyOn(useVaultStore.getState(), 'refreshDir').mockResolvedValue(undefined);
    const open = vi.spyOn(useWorkspaceStore.getState(), 'openFile').mockResolvedValue(undefined);
    expect(await toNote({ line: 2, time: '09:00', text: 'идея проекта' })).toBe(true);
    const noteCall = write.mock.calls.find((c) => c[0] === 'идея проекта.md');
    expect(noteCall?.[1]).toContain('# идея проекта');
    expect(open).toHaveBeenCalledWith('идея проекта.md');
    const inboxCall = write.mock.calls.find((c) => c[0] === INBOX);
    expect(inboxCall?.[1]).toBe('# Inbox\n- 10:00 b'); // строка вырезана
  });
});
