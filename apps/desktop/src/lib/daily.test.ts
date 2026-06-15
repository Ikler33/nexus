import { beforeEach, describe, expect, it } from 'vitest';
import {
  appendCapture,
  dailyNotePath,
  dateStamp,
  INBOX,
  openOrCreateDaily,
  openOrCreateInbox,
} from './daily';
import { tauriApi } from './tauri-api';
import { useVaultStore } from '../stores/vault';
import { activePath, useWorkspaceStore } from '../stores/workspace';

beforeEach(async () => {
  useWorkspaceStore.getState().reset();
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  await useVaultStore.getState().openVault('');
});

describe('daily (CAP-1)', () => {
  it('dateStamp/dailyNotePath форматируют YYYY-MM-DD с ведущими нулями', () => {
    const d = new Date(2026, 5, 9); // 9 июня 2026 (месяц 0-индексный)
    expect(dateStamp(d)).toBe('2026-06-09');
    expect(dailyNotePath(d)).toBe('Journal/2026-06-09.md');
  });

  it('openOrCreateDaily создаёт заметку дня из шаблона и открывает её', async () => {
    const d = new Date(2026, 5, 13);
    const path = await openOrCreateDaily(d);
    expect(path).toBe('Journal/2026-06-13.md');
    expect(activePath(useWorkspaceStore.getState())).toBe(path);
    const doc = useWorkspaceStore.getState().buffers[path]?.doc ?? '';
    expect(doc).toContain('# 2026-06-13'); // заголовок-дата из шаблона
  });

  it('повторный вызов открывает уже существующую заметку (не пересоздаёт)', async () => {
    const d = new Date(2026, 5, 13);
    await openOrCreateDaily(d);
    // правим и сохраняем, затем вновь открываем — содержимое не затёрто шаблоном
    const path = dailyNotePath(d);
    useWorkspaceStore.getState().updateBufferDoc(path, '# 2026-06-13\n\nмоя запись');
    await useWorkspaceStore.getState().saveBuffer(path, true);
    useWorkspaceStore.getState().reset();
    await openOrCreateDaily(d);
    const doc = useWorkspaceStore.getState().buffers[path]?.doc ?? '';
    expect(doc).toContain('моя запись');
  });

  it('appendCapture дозаписывает мысли в Inbox строкой с временем', async () => {
    await appendCapture('первая мысль', new Date(2026, 5, 13, 9, 5));
    await appendCapture('вторая', new Date(2026, 5, 13, 14, 30));
    const doc = await tauriApi.vault.readFile('Inbox.md');
    expect(doc).toContain('- 09:05 первая мысль');
    expect(doc).toContain('- 14:30 вторая');
    expect(doc.indexOf('первая')).toBeLessThan(doc.indexOf('вторая')); // порядок дозаписи
  });

  it('appendCapture с открытым Inbox пишет в буфер, не на диск', async () => {
    await appendCapture('первая', new Date(2026, 5, 13, 8, 0)); // Inbox создан на диске (буфера нет)
    await useWorkspaceStore.getState().openFile(INBOX); // теперь Inbox открыт в редакторе
    const diskBefore = await tauriApi.vault.readFile(INBOX);
    await appendCapture('вторая', new Date(2026, 5, 13, 9, 5)); // → должно уйти в буфер
    const buf = useWorkspaceStore.getState().buffers[INBOX]?.doc ?? '';
    expect(buf).toContain('- 09:05 вторая'); // буфер получил новую строку
    expect(await tauriApi.vault.readFile(INBOX)).toBe(diskBefore); // диск не тронут (иначе затёрли бы правки)
  });

  it('openOrCreateInbox открывает Inbox в редакторе (с содержимым)', async () => {
    await openOrCreateInbox();
    expect(activePath(useWorkspaceStore.getState())).toBe(INBOX);
    // Буфер загружен непустым: либо существующий Inbox, либо свежесозданный «# Inbox\n».
    expect((useWorkspaceStore.getState().buffers[INBOX]?.doc ?? '').length).toBeGreaterThan(0);
  });

  it('openOrCreateInbox не затирает существующий Inbox', async () => {
    await appendCapture('моя мысль', new Date(2026, 5, 13, 9, 5)); // Inbox уже есть на диске
    await openOrCreateInbox(); // открыть существующий — не пересоздавать пустым
    expect(useWorkspaceStore.getState().buffers[INBOX]?.doc ?? '').toContain('моя мысль');
  });
});
