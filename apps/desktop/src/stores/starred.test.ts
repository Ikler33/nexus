import { beforeEach, describe, expect, it } from 'vitest';

import { useStarredStore } from './starred';

beforeEach(() => {
  // localStorage в тест-окружении — частичный мок (нет .clear); persist в сторе обёрнут try/catch,
  // поэтому достаточно сбросить in-memory состояние стора между тестами.
  useStarredStore.setState({ paths: [] });
});

describe('starred store — переживает курацию (audit B9)', () => {
  it('rename переносит точный путь заметки и детей каталога', () => {
    useStarredStore.setState({ paths: ['Notes/A.md', 'Projects/X/y.md', 'Other.md'] });
    useStarredStore.getState().rename('Projects/X', 'Work/X'); // rename каталога
    expect(useStarredStore.getState().paths).toEqual(['Notes/A.md', 'Work/X/y.md', 'Other.md']);
    useStarredStore.getState().rename('Notes/A.md', 'Notes/B.md'); // rename файла
    expect(useStarredStore.getState().paths).toContain('Notes/B.md');
    expect(useStarredStore.getState().paths).not.toContain('Notes/A.md');
  });

  it('dropStarsUnder снимает звезду с пути и всех детей', () => {
    useStarredStore.setState({ paths: ['Folder/a.md', 'Folder/sub/b.md', 'Keep.md'] });
    useStarredStore.getState().dropStarsUnder('Folder');
    expect(useStarredStore.getState().paths).toEqual(['Keep.md']);
  });

  it('rename/drop не задевают префикс-похожие пути (Folder vs Folder2)', () => {
    useStarredStore.setState({ paths: ['Folder/a.md', 'Folder2/b.md'] });
    useStarredStore.getState().rename('Folder', 'X');
    expect(useStarredStore.getState().paths).toEqual(['X/a.md', 'Folder2/b.md']);
    useStarredStore.getState().dropStarsUnder('X');
    expect(useStarredStore.getState().paths).toEqual(['Folder2/b.md']);
  });
});
