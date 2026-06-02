import { beforeEach, describe, expect, it } from 'vitest';
import { useVaultStore } from './vault';
import { activeBuffer, activePath, useWorkspaceStore } from './workspace';

beforeEach(async () => {
  useWorkspaceStore.getState().reset();
  // notes нужны для openLink
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {}, notes: [] });
  await useVaultStore.getState().openVault('');
});

describe('workspace store (Ф0-9, Б12)', () => {
  it('openFile открывает буфер во вкладке и делает активным', async () => {
    await useWorkspaceStore.getState().openFile('Inbox.md');
    const s = useWorkspaceStore.getState();
    expect(activePath(s)).toBe('Inbox.md');
    expect(activeBuffer(s)?.path).toBe('Inbox.md');
    expect(activeBuffer(s)?.doc).toBeDefined();
    expect(s.groups[0].tabs).toContain('Inbox.md');
  });

  it('несколько вкладок в группе; переключение меняет активную', async () => {
    const ws = useWorkspaceStore.getState();
    await ws.openFile('README.md');
    await useWorkspaceStore.getState().openFile('Inbox.md');
    let s = useWorkspaceStore.getState();
    expect(s.groups[0].tabs).toEqual(['README.md', 'Inbox.md']);
    expect(activePath(s)).toBe('Inbox.md');

    useWorkspaceStore.getState().setActiveTab(s.activeGroupId, 'README.md');
    s = useWorkspaceStore.getState();
    expect(activePath(s)).toBe('README.md');
  });

  // AC-Б12-2: грязный буфер сохраняет dirty и содержимое при переключении вкладок.
  it('dirty и содержимое буфера переживают переключение вкладок', async () => {
    const ws = useWorkspaceStore.getState();
    await ws.openFile('README.md');
    await useWorkspaceStore.getState().openFile('Inbox.md');
    const gid = useWorkspaceStore.getState().activeGroupId;

    useWorkspaceStore.getState().setActiveTab(gid, 'README.md');
    useWorkspaceStore.getState().updateBufferDoc('README.md', 'edited content');
    expect(useWorkspaceStore.getState().buffers['README.md'].dirty).toBe(true);

    useWorkspaceStore.getState().setActiveTab(gid, 'Inbox.md'); // ушли
    useWorkspaceStore.getState().setActiveTab(gid, 'README.md'); // вернулись
    const buf = useWorkspaceStore.getState().buffers['README.md'];
    expect(buf.dirty).toBe(true);
    expect(buf.doc).toBe('edited content');
  });

  // AC-Б12-1: ≥2 группы (сплита); контекст — из активной группы.
  it('splitRight создаёт вторую группу; активный буфер — из активной группы', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().splitRight();
    let s = useWorkspaceStore.getState();
    expect(s.groups.length).toBe(2);
    expect(activePath(s)).toBe('README.md'); // сплит унёс активную вкладку

    // Открываем другой файл в новой (активной) группе.
    await useWorkspaceStore.getState().openFile('Inbox.md');
    s = useWorkspaceStore.getState();
    expect(activeBuffer(s)?.path).toBe('Inbox.md');

    // Переключаемся на первую группу — контекст меняется на её активную вкладку.
    useWorkspaceStore.getState().setActiveGroup(s.groups[0].id);
    expect(activePath(useWorkspaceStore.getState())).toBe('README.md');
  });

  it('saveBuffer сбрасывает dirty', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().updateBufferDoc('README.md', 'x');
    await useWorkspaceStore.getState().saveBuffer('README.md');
    expect(useWorkspaceStore.getState().buffers['README.md'].dirty).toBe(false);
  });

  it('closeTab убирает вкладку и держит хотя бы одну группу', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    const gid = useWorkspaceStore.getState().activeGroupId;
    useWorkspaceStore.getState().closeTab(gid, 'README.md');
    const s = useWorkspaceStore.getState();
    expect(s.groups.length).toBe(1);
    expect(s.groups[0].tabs).toHaveLength(0);
    expect(s.buffers['README.md']).toBeUndefined(); // GC
  });

  it('openLink резолвит wikilink и открывает файл', async () => {
    await useWorkspaceStore.getState().openLink('Projects/Roadmap');
    expect(activePath(useWorkspaceStore.getState())).toBe('Projects/Roadmap.md');
  });
});
