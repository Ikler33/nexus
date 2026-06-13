import { beforeEach, describe, expect, it } from 'vitest';
import { appendCapture, dailyNotePath, dateStamp, openOrCreateDaily } from './daily';
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
});
