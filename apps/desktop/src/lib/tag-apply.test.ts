import { beforeEach, describe, expect, it, vi } from 'vitest';

import { appendInlineTags, applyTags, existingInlineTags } from './tag-apply';
import * as fmEdit from './frontmatter-edit';
import { FlushFailedError } from './frontmatter-edit';
import { tauriApi } from './tauri-api';
import { useWorkspaceStore } from '../stores/workspace';

describe('tag-apply: чистая раскладка', () => {
  it('existingInlineTags находит #tag по границе слова, lowercase; не ловит # в середине; не цифро-токен', () => {
    const s = existingInlineTags('текст #Rust и #ai/ml #2024, но email@a#b не тег');
    expect(s.has('rust')).toBe(true);
    expect(s.has('ai/ml')).toBe(true);
    expect(s.has('b')).toBe(false); // a#b — # в середине слова
    expect(s.has('2024')).toBe(false); // цифро-токен — индексатор требует ≥1 букву
  });

  it('appendInlineTags дописывает недостающие, пропускает присутствующие/дубли, нормализует', () => {
    const r = appendInlineTags('тело\n\n#rust', ['#Rust', 'ai', 'ai', 'OPS']);
    expect(r.added).toEqual(['ai', 'ops']); // rust уже есть; ai дедуп; нормализация
    expect(r.content).toBe('тело\n\n#rust\n\n#ai #ops\n');
  });

  it('добавлять нечего → контент не меняется (идемпотентность)', () => {
    const r = appendInlineTags('тело #ai #rust', ['ai', '#RUST']);
    expect(r.added).toEqual([]);
    expect(r.content).toBe('тело #ai #rust');
  });

  it('пустое тело → строка тегов без ведущих пустых строк', () => {
    expect(appendInlineTags('   ', ['ai']).content).toBe('#ai\n');
  });
});

describe('tag-apply: applyTags (запись)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    useWorkspaceStore.setState({ buffers: {} });
  });

  it('флашит → читает → дописывает → пишет manual → sync; возвращает добавленные', async () => {
    const flush = vi.spyOn(fmEdit, 'flushBufferIfDirty').mockResolvedValue();
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('тело заметки');
    const write = vi.spyOn(tauriApi.vault, 'writeFile').mockResolvedValue('h9');
    const sync = vi.spyOn(useWorkspaceStore.getState(), 'syncBufferAfterWrite');

    const added = await applyTags('N.md', ['ai', 'rust']);

    expect(flush).toHaveBeenCalledWith('N.md');
    expect(write).toHaveBeenCalledWith('N.md', 'тело заметки\n\n#ai #rust\n', true);
    expect(sync).toHaveBeenCalledWith('N.md', 'тело заметки\n\n#ai #rust\n', 'h9');
    expect(added).toEqual(['ai', 'rust']);
  });

  it('все теги уже в теле → НЕ пишет (идемпотентно, без снапшота истории)', async () => {
    vi.spyOn(fmEdit, 'flushBufferIfDirty').mockResolvedValue();
    vi.spyOn(tauriApi.vault, 'readFile').mockResolvedValue('тело #ai #rust');
    const write = vi.spyOn(tauriApi.vault, 'writeFile');

    const added = await applyTags('N.md', ['ai', 'rust']);

    expect(write).not.toHaveBeenCalled();
    expect(added).toEqual([]);
  });

  it('флаш не удался → FlushFailedError, НЕ читаем и НЕ пишем (AI-1 R1 data-safety)', async () => {
    vi.spyOn(fmEdit, 'flushBufferIfDirty').mockRejectedValue(new FlushFailedError());
    const read = vi.spyOn(tauriApi.vault, 'readFile');
    const write = vi.spyOn(tauriApi.vault, 'writeFile');

    await expect(applyTags('N.md', ['ai'])).rejects.toBeInstanceOf(FlushFailedError);
    expect(read).not.toHaveBeenCalled();
    expect(write).not.toHaveBeenCalled();
  });
});
