import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { type Buffer, useWorkspaceStore } from '../../stores/workspace';
import { tauriApi, type TaskItem } from '../tauri-api';
import { collectTasks } from './collect';
import { toggleTaskInPlace } from './toggle';

/** Минимальные тест-буферы в стор (baseHash обязателен по типу Buffer). */
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

describe('collectTasks (TASK-1, буфер-aware)', () => {
  it('без грязных буферов возвращает дисковый список как есть', async () => {
    const disk: TaskItem[] = [{ path: 'a.md', line: 1, checked: false, text: 'x', title: 'A' }];
    vi.spyOn(tauriApi.tasks, 'listTasks').mockResolvedValue(disk);
    expect(await collectTasks()).toEqual(disk);
  });

  it('грязный буфер перекрывает задачи СВОЕГО файла (несохранённые правки), чужие — целы', async () => {
    const disk: TaskItem[] = [
      { path: 'a.md', line: 1, checked: false, text: 'старое', title: 'A' },
      { path: 'b.md', line: 1, checked: false, text: 'другой', title: 'B' },
    ];
    vi.spyOn(tauriApi.tasks, 'listTasks').mockResolvedValue(disk);
    setBuffers([{ path: 'a.md', doc: '- [x] новое\n- [ ] второе', dirty: true }]);
    const out = await collectTasks();
    expect(out.filter((t) => t.path === 'b.md')).toEqual([disk[1]]); // чужой файл не тронут
    expect(out.filter((t) => t.path === 'a.md')).toEqual([
      { path: 'a.md', line: 1, checked: true, text: 'новое', title: 'A' }, // title из индекса сохранён
      { path: 'a.md', line: 2, checked: false, text: 'второе', title: 'A' },
    ]);
  });

  it('ЧИСТЫЙ (не dirty) буфер НЕ перекрывает диск', async () => {
    const disk: TaskItem[] = [{ path: 'a.md', line: 1, checked: false, text: 'диск', title: 'A' }];
    vi.spyOn(tauriApi.tasks, 'listTasks').mockResolvedValue(disk);
    setBuffers([{ path: 'a.md', doc: '- [ ] буфер', dirty: false }]);
    expect(await collectTasks()).toEqual(disk);
  });
});

describe('toggleTaskInPlace (TASK-1)', () => {
  it('открытый буфер — источник правды: тоггл через updateBufferDoc + dirty', async () => {
    setBuffers([{ path: 'a.md', doc: '- [ ] дело', dirty: false }]);
    expect(await toggleTaskInPlace('a.md', 1)).toBe(true);
    const buf = useWorkspaceStore.getState().buffers['a.md'];
    expect(buf.doc).toBe('- [x] дело');
    expect(buf.dirty).toBe(true);
  });

  it('закрытый файл: читает диск, тоггл, пишет (manual=false)', async () => {
    const read = vi
      .spyOn(tauriApi.vault, 'readFileMeta')
      .mockResolvedValue({ content: '- [ ] z', hash: 'h' });
    const write = vi.spyOn(tauriApi.vault, 'writeFile').mockResolvedValue('h2');
    expect(await toggleTaskInPlace('closed.md', 1)).toBe(true);
    expect(read).toHaveBeenCalledWith('closed.md');
    expect(write).toHaveBeenCalledWith('closed.md', '- [x] z', false);
  });

  it('дрейф (строка уже не таск) → false, без записи', async () => {
    setBuffers([{ path: 'a.md', doc: 'обычный текст', dirty: false }]);
    expect(await toggleTaskInPlace('a.md', 1)).toBe(false);
    expect(useWorkspaceStore.getState().buffers['a.md'].doc).toBe('обычный текст');
  });
});
