import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tauriApi } from '../lib/tauri-api';
import { flushAllDirty } from './autosave';
import { usePrefsStore } from './prefs';
import { useUIStore } from './ui';
import { useVaultStore } from './vault';
import { activeBuffer, activePath, useWorkspaceStore } from './workspace';

beforeEach(async () => {
  useWorkspaceStore.getState().reset();
  // openLink резолвится через tauriApi.vault.resolveNote (#22) — вне Tauri отвечает мок.
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  await useVaultStore.getState().openVault('');
});

afterEach(() => {
  // P0-2: openFile дёргает useUIStore (closeHome/News/Board/Today/Agent) — возвращаем main-вью к дефолту,
  // чтобы флаги не текли между тестами/файлами.
  useUIStore.setState({
    homeOpen: true,
    newsOpen: false,
    boardOpen: false,
    todayOpen: false,
    agentOpen: false,
  });
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

  // P0-2: открытие файла гасит ВСЕ полноэкранные main-вьюхи (Home/News/Board/Today/Agent), иначе
  // редактор откроется ЗА Today/Agent = «мёртвый клик» из дерева/палитры/⌘O (раньше гасили только
  // Home/News/Board — Today/Agent забыли).
  it('openFile гасит todayOpen и agentOpen (anti-dead-click)', async () => {
    useUIStore.setState({ todayOpen: true, agentOpen: true, homeOpen: false });
    await useWorkspaceStore.getState().openFile('Inbox.md');
    expect(useUIStore.getState().todayOpen).toBe(false);
    expect(useUIStore.getState().agentOpen).toBe(false);
    expect(useUIStore.getState().homeOpen).toBe(false);
    // Файл реально стал активным (редактор впереди).
    expect(activePath(useWorkspaceStore.getState())).toBe('Inbox.md');
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

  // W-1: крестик закрытия пейна. closeGroup убирает группу, активной делает оставшуюся, GC буферов.
  it('closeGroup закрывает пейн, перецеливает активную группу, GC-ит осиротевший буфер', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().splitRight();
    await useWorkspaceStore.getState().openFile('Inbox.md'); // во второй (активной) группе
    let s = useWorkspaceStore.getState();
    expect(s.groups.length).toBe(2);
    const secondId = s.activeGroupId;
    expect(activePath(s)).toBe('Inbox.md');

    useWorkspaceStore.getState().closeGroup(secondId);
    s = useWorkspaceStore.getState();
    expect(s.groups.length).toBe(1);
    expect(s.groups.some((g) => g.id === secondId)).toBe(false);
    expect(s.activeGroupId).toBe(s.groups[0].id); // активной стала оставшаяся группа
    expect(activePath(s)).toBe('README.md');
    // Inbox.md больше нигде не открыт → буфер собран GC.
    expect(s.buffers['Inbox.md']).toBeUndefined();
    // README.md ещё открыт в оставшейся группе → буфер жив.
    expect(s.buffers['README.md']).toBeDefined();
  });

  it('closeGroup никогда не закрывает последнюю группу (no-op)', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    const s0 = useWorkspaceStore.getState();
    expect(s0.groups.length).toBe(1);
    useWorkspaceStore.getState().closeGroup(s0.activeGroupId);
    const s = useWorkspaceStore.getState();
    expect(s.groups.length).toBe(1);
    expect(activePath(s)).toBe('README.md'); // ничего не потеряли
  });

  // Ревью-major: закрытие АКТИВНОЙ группы меняет активный документ → курсор истории нужно снапнуть
  // на запись реально активного пути (инвариант navHistory[navIndex].path === activePath), иначе
  // back «молча» жёг бы шаг.
  it('closeGroup активной группы realign-ит navIndex на активный путь (back/forward консистентны)', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().splitRight();
    await useWorkspaceStore.getState().openFile('Inbox.md'); // во второй (активной) группе
    let s = useWorkspaceStore.getState();
    const secondId = s.activeGroupId;
    expect(activePath(s)).toBe('Inbox.md');

    useWorkspaceStore.getState().closeGroup(secondId);
    s = useWorkspaceStore.getState();
    expect(activePath(s)).toBe('README.md');
    // Инвариант: курсор истории указывает на реально активный документ.
    expect(s.navHistory[s.navIndex]?.path).toBe(activePath(s));
  });

  // Ревью-minor: при закрытии активной группы фолбэк целится в первую НЕПУСТУЮ из оставшихся,
  // а не в groups[0] (который может быть пустым пейном) — иначе редактор зря пустеет.
  it('closeGroup активной группы → фолбэк на непустой пейн, не на пустой groups[0]', async () => {
    // g0 остаётся пустым; README открыт в g1; g2 активна и закрывается.
    useWorkspaceStore.getState().splitRight(); // g1 активна (пустая)
    await useWorkspaceStore.getState().openFile('README.md'); // README в g1
    useWorkspaceStore.getState().splitRight(); // g2 активна (копия README)
    let s = useWorkspaceStore.getState();
    expect(s.groups.length).toBe(3);
    const g0 = s.groups[0].id;
    const g1 = s.groups[1].id;
    const g2 = s.activeGroupId;
    expect(s.groups.find((g) => g.id === g0)?.tabs.length).toBe(0); // g0 пустой
    expect(s.groups.find((g) => g.id === g1)?.tabs).toContain('README.md');

    useWorkspaceStore.getState().closeGroup(g2);
    s = useWorkspaceStore.getState();
    // Активной стала НЕпустая g1 (с README), а не пустая g0.
    expect(s.activeGroupId).toBe(g1);
    expect(activePath(s)).toBe('README.md');
  });

  // SAFE-4: закрытие пейна с НЕсохранённым буфером флашит его на диск ПЕРЕД GC (как closeTab).
  it('closeGroup флашит несохранённый буфер закрываемого пейна (SAFE-4)', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().splitRight();
    await useWorkspaceStore.getState().openFile('Inbox.md');
    useWorkspaceStore.getState().updateBufferDoc('Inbox.md', 'unsaved edit before close');
    const writeSpy = vi.spyOn(tauriApi.vault, 'writeFile');
    const secondId = useWorkspaceStore.getState().activeGroupId;
    await useWorkspaceStore.getState().closeGroup(secondId); // P0-5: async — ждём flush осиротевшего буфера
    expect(writeSpy).toHaveBeenCalledWith('Inbox.md', 'unsaved edit before close', false);
    // Запись прошла → пейн закрыт, буфер собран GC.
    expect(useWorkspaceStore.getState().groups.length).toBe(1);
    expect(useWorkspaceStore.getState().buffers['Inbox.md']).toBeUndefined();
    writeSpy.mockRestore();
  });

  // P0-5 (DATA-SAFETY, ядро среза): закрытие пейна, чей осиротевший буфер НЕ записался (writeFile
  // reject) — пейн НЕ закрывается, saveError виден, буфер ЦЕЛ (правки не потеряны).
  it('closeGroup при ОШИБКЕ записи осиротевшего буфера НЕ закрывает пейн (правки целы)', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().splitRight();
    await useWorkspaceStore.getState().openFile('Inbox.md');
    useWorkspaceStore.getState().updateBufferDoc('Inbox.md', 'правки под угрозой');
    const writeSpy = vi
      .spyOn(tauriApi.vault, 'writeFile')
      .mockRejectedValueOnce(new Error('диск полон'));
    const secondId = useWorkspaceStore.getState().activeGroupId;
    await useWorkspaceStore.getState().closeGroup(secondId);
    const s = useWorkspaceStore.getState();
    // Пейн остался открыт.
    expect(s.groups.some((g) => g.id === secondId)).toBe(true);
    expect(s.groups.length).toBe(2);
    // Буфер цел, правки на месте, saveError видим, dirty НЕ сброшен.
    expect(s.buffers['Inbox.md']).toBeDefined();
    expect(s.buffers['Inbox.md'].doc).toBe('правки под угрозой');
    expect(s.buffers['Inbox.md'].dirty).toBe(true);
    expect(s.buffers['Inbox.md'].saveError).toContain('диск полон');
    writeSpy.mockRestore();
  });

  it('saveBuffer сбрасывает dirty', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().updateBufferDoc('README.md', 'x');
    await useWorkspaceStore.getState().saveBuffer('README.md');
    expect(useWorkspaceStore.getState().buffers['README.md'].dirty).toBe(false);
  });

  // SAFE-2: baseHash — отпечаток диска; заполняется при open, обновляется при save (для guard SAFE-3).
  it('openFile заполняет baseHash, saveBuffer его обновляет', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    const opened = useWorkspaceStore.getState().buffers['README.md'];
    expect(opened.baseHash).toBeTruthy();

    useWorkspaceStore.getState().updateBufferDoc('README.md', '# Изменено\n');
    await useWorkspaceStore.getState().saveBuffer('README.md');
    const saved = useWorkspaceStore.getState().buffers['README.md'];
    expect(saved.dirty).toBe(false);
    expect(saved.baseHash).not.toBe(opened.baseHash); // новый контент → новый baseHash
    expect(saved.baseHash).toBe(await tauriApi.vault.fileHash('README.md'));
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

  it('openLink на несуществующую заметку создаёт её и открывает (anti-dead-click)', async () => {
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue(null); // заметки нет
    const { useVaultStore } = await import('./vault');
    const create = vi.spyOn(useVaultStore.getState(), 'createNote').mockResolvedValue('Новая идея.md');
    await useWorkspaceStore.getState().openLink('Новая идея');
    expect(create).toHaveBeenCalledWith('', { baseName: 'Новая идея' });
    expect(activePath(useWorkspaceStore.getState())).toBe('Новая идея.md');
  });

  it('openLink на [[folder/note]] создаёт заметку в подкаталоге', async () => {
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue(null);
    const { useVaultStore } = await import('./vault');
    const create = vi.spyOn(useVaultStore.getState(), 'createNote').mockResolvedValue('Проекты/Идея.md');
    await useWorkspaceStore.getState().openLink('Проекты/Идея');
    expect(create).toHaveBeenCalledWith('Проекты', { baseName: 'Идея' });
  });

  it('openLink с пустым/мусорным target не создаёт заметку', async () => {
    vi.spyOn(tauriApi.vault, 'resolveNote').mockResolvedValue(null);
    const { useVaultStore } = await import('./vault');
    const create = vi.spyOn(useVaultStore.getState(), 'createNote');
    await useWorkspaceStore.getState().openLink('   ');
    await useWorkspaceStore.getState().openLink('***'); // только недопустимые символы → пусто
    expect(create).not.toHaveBeenCalled();
  });

  // DP-3: DnD вкладок между панами — без дублей, буфер жив, пустая группа схлопывается.
  it('moveTab переносит вкладку между группами и схлопывает пустую', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().splitRight(); // вторая группа с тем же табом
    const s0 = useWorkspaceStore.getState();
    const [g1, g2] = s0.groups.map((g) => g.id);
    await useWorkspaceStore.getState().openFile('Inbox.md', g1);

    // Перенос Inbox.md из g1 в g2: появилась в g2 (активной), из g1 ушла, буфер жив.
    useWorkspaceStore.getState().moveTab(g1, g2, 'Inbox.md');
    let s = useWorkspaceStore.getState();
    expect(s.groups.find((g) => g.id === g2)?.tabs).toContain('Inbox.md');
    expect(s.groups.find((g) => g.id === g1)?.tabs).not.toContain('Inbox.md');
    expect(s.activeGroupId).toBe(g2);
    expect(s.buffers['Inbox.md']).toBeDefined();

    // Перенос дубля (README есть в обеих): в цели НЕ дублируется, источник пустеет и схлопывается.
    useWorkspaceStore.getState().moveTab(g1, g2, 'README.md');
    s = useWorkspaceStore.getState();
    expect(s.groups).toHaveLength(1);
    expect(s.groups[0].tabs.filter((p) => p === 'README.md')).toHaveLength(1);
  });

  // audit B8: записи истории навигации перемещённой вкладки должны указывать на новую группу —
  // иначе back/forward открыл бы её копию в старой группе (где её уже нет).
  it('moveTab перецеливает navHistory перемещённой вкладки на новую группу', async () => {
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().splitRight();
    const [g1, g2] = useWorkspaceStore.getState().groups.map((g) => g.id);
    await useWorkspaceStore.getState().openFile('Inbox.md', g1); // navHistory: {Inbox.md, g1}
    expect(
      useWorkspaceStore.getState().navHistory.some((e) => e.path === 'Inbox.md' && e.groupId === g1),
    ).toBe(true);

    useWorkspaceStore.getState().moveTab(g1, g2, 'Inbox.md');
    const nav = useWorkspaceStore.getState().navHistory;
    expect(nav.some((e) => e.path === 'Inbox.md' && e.groupId === g1)).toBe(false);
    expect(nav.some((e) => e.path === 'Inbox.md' && e.groupId === g2)).toBe(true);
    // README.md (был в обеих группах, его запись истории — в активной) не задет лишним ремапом.
    expect(nav.some((e) => e.path === 'README.md')).toBe(true);
  });

  // DP-3: режим source/preview — пер-группный, toggleMode без аргумента бьёт по активной.
  it('toggleMode переключает режим активной группы', async () => {
    usePrefsStore.setState({ noteMode: 'source' }); // изоляция от порядка тестов (преф персистится)
    await useWorkspaceStore.getState().openFile('README.md');
    const gid = useWorkspaceStore.getState().activeGroupId;
    expect(useWorkspaceStore.getState().modes[gid]).toBeUndefined(); // дефолт source (из префа)
    useWorkspaceStore.getState().toggleMode();
    expect(useWorkspaceStore.getState().modes[gid]).toBe('preview');
    useWorkspaceStore.getState().toggleMode(gid);
    expect(useWorkspaceStore.getState().modes[gid]).toBe('source');
  });

  // ── EDFIX-4 F4: последний выбранный режим — персист-дефолт для новых панелей/запуска. ──
  it('EDFIX-4: toggleMode персистит выбранный режим в преф noteMode (+localStorage)', async () => {
    usePrefsStore.setState({ noteMode: 'source' });
    await useWorkspaceStore.getState().openFile('README.md');
    useWorkspaceStore.getState().toggleMode();
    expect(usePrefsStore.getState().noteMode).toBe('preview');
    expect(localStorage.getItem('nexus.editor.noteMode')).toBe('preview');
    useWorkspaceStore.getState().toggleMode();
    expect(usePrefsStore.getState().noteMode).toBe('source');
    expect(localStorage.getItem('nexus.editor.noteMode')).toBe('source');
  });

  it('EDFIX-4: новая группа (без записи в modes) наследует преф; первый toggle идёт ОТ префа', async () => {
    usePrefsStore.setState({ noteMode: 'preview' });
    await useWorkspaceStore.getState().openFile('README.md');
    const gid = useWorkspaceStore.getState().activeGroupId;
    // Записи в modes нет → эффективный режим группы = преф ('preview', читает GroupPane).
    expect(useWorkspaceStore.getState().modes[gid]).toBeUndefined();
    // Первый toggle отталкивается от префа: preview → source (а не от хардкода 'source' → preview).
    useWorkspaceStore.getState().toggleMode();
    expect(useWorkspaceStore.getState().modes[gid]).toBe('source');
    expect(usePrefsStore.getState().noteMode).toBe('source');
  });

  it('EDFIX-4: toggleMode НЕ трогает открытые панели ретроактивно — их режим фиксируется в modes', async () => {
    usePrefsStore.setState({ noteMode: 'source' });
    await useWorkspaceStore.getState().openFile('README.md');
    const g1 = useWorkspaceStore.getState().activeGroupId;
    useWorkspaceStore.getState().splitRight();
    const g2 = useWorkspaceStore.getState().activeGroupId;
    expect(g2).not.toBe(g1);
    // Обе группы без явной записи (наследуют 'source'). Тогглим ТОЛЬКО g2 → преф станет 'preview',
    // но g1 обязан ОСТАТЬСЯ в source: его унаследованный режим фиксируется в modes до смены префа.
    useWorkspaceStore.getState().toggleMode(g2);
    const s = useWorkspaceStore.getState();
    expect(s.modes[g2]).toBe('preview');
    expect(s.modes[g1]).toBe('source'); // зафиксирован, не уехал за префом
    expect(usePrefsStore.getState().noteMode).toBe('preview');
  });

  // ревью MINOR-3: схлопывание групп обязано чистить modes — freshGroups() переиспользует id 'g0',
  // и stale-запись воскрешённого g0 перебила бы преф noteMode.
  it('EDFIX-4: воскрешённый g0 (closeTab опустошил все группы) берёт режим из ПРЕФА, не из stale-записи', async () => {
    usePrefsStore.setState({ noteMode: 'source' });
    await useWorkspaceStore.getState().openFile('README.md');
    const gid = useWorkspaceStore.getState().activeGroupId;
    useWorkspaceStore.getState().toggleMode(); // modes[gid]='preview', преф='preview'
    expect(useWorkspaceStore.getState().modes[gid]).toBe('preview');
    // Преф разошёлся со stale-записью (владелец позже выбрал source в другом месте).
    usePrefsStore.setState({ noteMode: 'source' });
    await useWorkspaceStore.getState().closeTab(gid, 'README.md'); // группа пустеет → freshGroups()
    const s = useWorkspaceStore.getState();
    expect(s.groups[0].id).toBe(gid); // id 'g0' переиспользован freshGroups()
    expect(s.modes[gid]).toBeUndefined(); // stale-запись вычищена → эффективный режим = преф ('source')
  });

  it('EDFIX-4: moveTab схлопывает опустевшую группу → её запись в modes вычищается (выжившая цела)', async () => {
    usePrefsStore.setState({ noteMode: 'source' });
    await useWorkspaceStore.getState().openFile('README.md');
    const g1 = useWorkspaceStore.getState().activeGroupId;
    useWorkspaceStore.getState().splitRight(); // g2 с той же вкладкой
    const g2 = useWorkspaceStore.getState().activeGroupId;
    useWorkspaceStore.getState().toggleMode(g2); // modes: g2='preview', g1 зафиксирован 'source'
    expect(useWorkspaceStore.getState().modes[g1]).toBe('source');
    useWorkspaceStore.getState().moveTab(g2, g1, 'README.md'); // g2 пустеет → схлопывается
    const s = useWorkspaceStore.getState();
    expect(s.groups.some((g) => g.id === g2)).toBe(false);
    expect(s.modes[g2]).toBeUndefined(); // запись схлопнутой группы вычищена
    expect(s.modes[g1]).toBe('source'); // запись выжившей группы цела
  });
});

describe('workspace external-change guard (SAFE-3)', () => {
  const ws = () => useWorkspaceStore.getState();

  it('эхо своего сейва (hash === baseHash) игнорируется', async () => {
    await ws().openFile('README.md');
    const b0 = ws().buffers['README.md'];
    await ws().onExternalFileChange('README.md', b0.baseHash);
    const b1 = ws().buffers['README.md'];
    expect(b1.externalChange).toBeFalsy();
    expect(b1.doc).toBe(b0.doc);
  });

  it('чистый буфер + внешнее изменение → тихий reload с диска', async () => {
    await ws().openFile('README.md');
    const newHash = await tauriApi.vault.writeFile('README.md', '# Снаружи изменено\n');
    await ws().onExternalFileChange('README.md', newHash);
    const b = ws().buffers['README.md'];
    expect(b.doc).toBe('# Снаружи изменено\n');
    expect(b.baseHash).toBe(newHash);
    expect(b.dirty).toBe(false);
    expect(b.externalChange).toBeFalsy();
  });

  it('грязный буфер + внешнее изменение → баннер, правки целы', async () => {
    await ws().openFile('README.md');
    ws().updateBufferDoc('README.md', 'мои несохранённые правки');
    await ws().onExternalFileChange('README.md', 'совсем-другой-хеш');
    const b = ws().buffers['README.md'];
    expect(b.externalChange).toBe(true);
    expect(b.doc).toBe('мои несохранённые правки'); // не затёрты
    expect(b.dirty).toBe(true);
  });

  it('keepMine снимает баннер и двигает baseHash к диску, правки целы', async () => {
    await ws().openFile('README.md');
    ws().updateBufferDoc('README.md', 'мои правки');
    const diskHash = await tauriApi.vault.writeFile('README.md', '# Версия с диска\n');
    await ws().onExternalFileChange('README.md', diskHash);
    expect(ws().buffers['README.md'].externalChange).toBe(true);
    await ws().keepMine('README.md');
    const b = ws().buffers['README.md'];
    expect(b.externalChange).toBe(false);
    expect(b.baseHash).toBe(diskHash);
    expect(b.doc).toBe('мои правки');
  });
});

describe('workspace autosave + flush (SAFE-4)', () => {
  const ws = () => useWorkspaceStore.getState();

  it('updateBufferDoc планирует автосейв через паузу набора (debounce)', async () => {
    await ws().openFile('README.md');
    const spy = vi.spyOn(tauriApi.vault, 'writeFile');
    vi.useFakeTimers();
    try {
      ws().updateBufferDoc('README.md', 'набор');
      expect(spy).not.toHaveBeenCalled(); // пауза ещё не прошла
      vi.advanceTimersByTime(1000);
      expect(spy).toHaveBeenCalledWith('README.md', 'набор', false); // автосейв = не ручной
    } finally {
      vi.useRealTimers();
      spy.mockRestore();
    }
  });

  it('closeTab флашит грязный буфер ПЕРЕД GC (нет потери правок)', async () => {
    await ws().openFile('README.md');
    ws().updateBufferDoc('README.md', 'важная правка');
    const spy = vi.spyOn(tauriApi.vault, 'writeFile');
    const gid = ws().activeGroupId;
    await ws().closeTab(gid, 'README.md'); // P0-5: async — ждём flush осиротевшего буфера до GC
    expect(spy).toHaveBeenCalledWith('README.md', 'важная правка', false);
    // Запись прошла → вкладка закрыта, буфер собран GC.
    expect(ws().groups[0].tabs).not.toContain('README.md');
    expect(ws().buffers['README.md']).toBeUndefined();
    spy.mockRestore();
  });

  // P0-5 (DATA-SAFETY, ядро среза): закрытие вкладки, чей осиротевший буфер НЕ записался — вкладка
  // НЕ удаляется, буфер ЦЕЛ, saveError виден. Раньше fire-and-forget flush + синхронный GC = тихая
  // потеря правок (catch saveBuffer не находил буфер).
  it('closeTab при ОШИБКЕ записи осиротевшего буфера НЕ закрывает вкладку (правки целы)', async () => {
    await ws().openFile('README.md');
    ws().updateBufferDoc('README.md', 'правки под угрозой');
    const spy = vi
      .spyOn(tauriApi.vault, 'writeFile')
      .mockRejectedValueOnce(new Error('диск полон'));
    const gid = ws().activeGroupId;
    await ws().closeTab(gid, 'README.md');
    const s = ws();
    // Вкладка осталась.
    expect(s.groups[0].tabs).toContain('README.md');
    // Буфер цел, правки на месте, saveError виден, dirty НЕ сброшен.
    expect(s.buffers['README.md']).toBeDefined();
    expect(s.buffers['README.md'].doc).toBe('правки под угрозой');
    expect(s.buffers['README.md'].dirty).toBe(true);
    expect(s.buffers['README.md'].saveError).toContain('диск полон');
    spy.mockRestore();
  });

  // P0-5: грязный буфер, открытый ТАКЖЕ в другой группе, при закрытии одной вкладки НЕ осиротевает →
  // flush fire-and-forget (happy-path SAFE-4), вкладка закрывается, буфер жив (GC его не трогает).
  it('closeTab не-осиротевшего буфера (открыт в др. группе) закрывает вкладку, буфер жив', async () => {
    await ws().openFile('README.md');
    ws().splitRight(); // README теперь в двух группах (g0, g1)
    ws().updateBufferDoc('README.md', 'правка');
    const [g0] = ws().groups.map((g) => g.id);
    await ws().closeTab(g0, 'README.md');
    // README больше не в g0 (g0 опустел и схлопнулся), но буфер жив — README остался в g1 → GC не собрал.
    expect(ws().groups.flatMap((g) => g.tabs)).toContain('README.md'); // ещё открыт в g1
    expect(ws().buffers['README.md']).toBeDefined();
    expect(ws().buffers['README.md'].doc).toBe('правка');
  });

  it('flushAllDirty сохраняет все грязные буферы', async () => {
    await ws().openFile('README.md');
    await ws().openFile('Inbox.md');
    ws().updateBufferDoc('README.md', 'a');
    ws().updateBufferDoc('Inbox.md', 'b');
    const spy = vi.spyOn(tauriApi.vault, 'writeFile');
    await flushAllDirty();
    expect(spy).toHaveBeenCalledWith('README.md', 'a', false);
    expect(spy).toHaveBeenCalledWith('Inbox.md', 'b', false);
    expect(ws().buffers['README.md'].dirty).toBe(false);
    expect(ws().buffers['Inbox.md'].dirty).toBe(false);
    spy.mockRestore();
  });

  it('ошибка сохранения: dirty остаётся, saveError виден, правки целы', async () => {
    await ws().openFile('README.md');
    ws().updateBufferDoc('README.md', 'правка');
    const spy = vi
      .spyOn(tauriApi.vault, 'writeFile')
      .mockRejectedValueOnce(new Error('диск полон'));
    await ws().saveBuffer('README.md');
    const b = ws().buffers['README.md'];
    expect(b.dirty).toBe(true); // не теряем правки
    expect(b.saveError).toContain('диск полон');
    expect(b.doc).toBe('правка');
    expect(b.saving).toBe(false);
    spy.mockRestore();
  });
});

describe('workspace dropPathsUnder (CURATE-1)', () => {
  const ws = () => useWorkspaceStore.getState();

  it('выбрасывает буфер и вкладку удалённого файла, сосед цел', async () => {
    await ws().openFile('README.md');
    await ws().openFile('Inbox.md');
    expect(ws().buffers['README.md']).toBeDefined();
    ws().dropPathsUnder('README.md');
    const s = ws();
    expect(s.buffers['README.md']).toBeUndefined();
    expect(s.groups.flatMap((g) => g.tabs)).not.toContain('README.md');
    expect(s.buffers['Inbox.md']).toBeDefined();
  });

  it('выбрасывает все буферы поддерева удалённого каталога', async () => {
    await ws().openFile('Projects/Roadmap.md');
    expect(ws().buffers['Projects/Roadmap.md']).toBeDefined();
    ws().dropPathsUnder('Projects');
    expect(ws().buffers['Projects/Roadmap.md']).toBeUndefined();
  });
});

describe('workspace renameBufferPath (CURATE-2)', () => {
  const ws = () => useWorkspaceStore.getState();

  it('переносит буфер и вкладку на новый путь файла', async () => {
    await ws().openFile('README.md');
    ws().updateBufferDoc('README.md', 'текст');
    ws().renameBufferPath('README.md', 'Renamed.md');
    const s = ws();
    expect(s.buffers['README.md']).toBeUndefined();
    expect(s.buffers['Renamed.md']).toBeDefined();
    expect(s.buffers['Renamed.md'].path).toBe('Renamed.md');
    expect(s.buffers['Renamed.md'].doc).toBe('текст'); // содержимое сохранено
    expect(s.groups.flatMap((g) => g.tabs)).toContain('Renamed.md');
  });

  it('переносит всё поддерево при rename каталога', async () => {
    await ws().openFile('Projects/Roadmap.md');
    ws().renameBufferPath('Projects', 'Plans');
    const s = ws();
    expect(s.buffers['Projects/Roadmap.md']).toBeUndefined();
    expect(s.buffers['Plans/Roadmap.md']).toBeDefined();
  });

  // NAV-2: недавние заметки для ⌘O quick-switcher.
  it('pushRecent: MRU-порядок без дублей', () => {
    ws().pushRecent('A.md');
    ws().pushRecent('B.md');
    ws().pushRecent('A.md'); // повтор — поднимается наверх, не дублируется
    expect(ws().recents).toEqual(['A.md', 'B.md']);
  });

  it('pushRecent: кап 20 (старейшее выбрасывается)', () => {
    for (let i = 0; i < 25; i++) ws().pushRecent(`N${i}.md`);
    const r = ws().recents;
    expect(r).toHaveLength(20);
    expect(r[0]).toBe('N24.md'); // последнее открытое — первое
    expect(r).not.toContain('N4.md'); // самые старые вытеснены
  });

  it('openFile добавляет путь в недавние', async () => {
    await ws().openFile('Inbox.md');
    expect(ws().recents[0]).toBe('Inbox.md');
  });

  // NAV-3: история навигации back/forward (⌘[ / ⌘]).
  const navPaths = () => ws().navHistory.map((e) => e.path);

  it('openFile пишет историю (путь+группа); navBack/navForward ходят по ней', async () => {
    await ws().openFile('README.md');
    await ws().openFile('Inbox.md');
    expect(ws().navIndex).toBe(1);
    expect(navPaths()).toEqual(['README.md', 'Inbox.md']);
    expect(ws().navHistory[0].groupId).toBe(ws().activeGroupId); // группа записана

    await ws().navBack();
    expect(ws().navIndex).toBe(0);
    expect(activePath(ws())).toBe('README.md');

    await ws().navForward();
    expect(ws().navIndex).toBe(1);
    expect(activePath(ws())).toBe('Inbox.md');
  });

  it('navBack на левом краю — no-op; новое открытие после back обрезает «вперёд»', async () => {
    await ws().openFile('A.md');
    await ws().openFile('B.md');
    await ws().navBack(); // → A, index 0
    await ws().navBack(); // край (index 0) — no-op
    expect(ws().navIndex).toBe(0);
    expect(activePath(ws())).toBe('A.md');

    await ws().openFile('C.md'); // новая навигация из середины — хвост [B] отброшен
    expect(navPaths()).toEqual(['A.md', 'C.md']);
    expect(ws().navIndex).toBe(1);
  });

  it('navForward на правом краю — no-op', async () => {
    await ws().openFile('A.md');
    await ws().openFile('B.md'); // idx 1 = правый край
    await ws().navForward();
    expect(ws().navIndex).toBe(1);
    expect(activePath(ws())).toBe('B.md');
  });

  it('navBack не плодит запись в истории (fromNav)', async () => {
    await ws().openFile('A.md');
    await ws().openFile('B.md');
    await ws().navBack();
    expect(navPaths()).toEqual(['A.md', 'B.md']); // длина не выросла
    expect(ws().navIndex).toBe(0);
  });

  it('переключение вкладки (setActiveTab) пишется в историю', async () => {
    await ws().openFile('A.md');
    await ws().openFile('B.md'); // [A,B] idx1
    ws().setActiveTab(ws().activeGroupId, 'A.md'); // клик по уже открытой вкладке A — навигация
    expect(navPaths()).toEqual(['A.md', 'B.md', 'A.md']);
    expect(ws().navIndex).toBe(2);
  });

  it('кап истории NAV_MAX=50 (старейшее выбрасывается)', async () => {
    for (let i = 0; i < 55; i++) await ws().openFile(`H${i}.md`);
    expect(ws().navHistory).toHaveLength(50);
    expect(ws().navIndex).toBe(49);
    expect(navPaths()[0]).toBe('H5.md'); // H0..H4 вытеснены
    expect(navPaths()).not.toContain('H4.md');
  });

  // Пересечение с CURATE-1/2: история не должна держать мёртвые пути.
  it('удаление пути чистит историю и сдвигает курсор (dropPathsUnder)', async () => {
    await ws().openFile('A.md');
    await ws().openFile('B.md');
    await ws().openFile('C.md'); // [A,B,C] idx2
    ws().dropPathsUnder('B.md'); // удалили B
    expect(navPaths()).toEqual(['A.md', 'C.md']);
    expect(ws().navIndex).toBe(1); // курсор сдвинут на число удалённых слева-включительно
  });

  // Регресс на находку ревью: удаление записи НА курсоре с выжившими по обе стороны.
  it('удаление активной записи держит курсор на реально активном документе', async () => {
    await ws().openFile('A.md');
    await ws().openFile('B.md');
    await ws().openFile('C.md'); // [A,B,C] idx2
    await ws().navBack(); // → B, idx1 (B активна и в центре истории)
    ws().dropPathsUnder('B.md'); // удаляем B — activeTab уходит на правого выжившего (C)
    // ИНВАРИАНТ: запись под курсором == реально открытый документ.
    expect(ws().navHistory[ws().navIndex].path).toBe(activePath(ws()));
    expect(activePath(ws())).toBe('C.md');
    await ws().navBack(); // и назад реально достижим A
    expect(activePath(ws())).toBe('A.md');
  });

  it('удаление чистит и недавние (recents)', async () => {
    await ws().openFile('A.md');
    await ws().openFile('B.md');
    ws().dropPathsUnder('A.md');
    expect(ws().recents).not.toContain('A.md');
    expect(ws().recents).toContain('B.md');
  });

  it('переименование пути ремапит историю (renameBufferPath)', async () => {
    await ws().openFile('A.md');
    await ws().openFile('Old.md'); // [A,Old] idx1
    ws().renameBufferPath('Old.md', 'New.md');
    expect(navPaths()).toEqual(['A.md', 'New.md']);
    expect(ws().navIndex).toBe(1); // длина/порядок сохранены
  });

  it('переименование ремапит и недавние (recents)', async () => {
    await ws().openFile('Old.md');
    ws().renameBufferPath('Old.md', 'New.md');
    expect(ws().recents).toContain('New.md');
    expect(ws().recents).not.toContain('Old.md');
  });

  it('navBack на недоступную цель (reject openFile) не рассинхронит курсор', async () => {
    await ws().openFile('A.md');
    await ws().openFile('B.md'); // [A,B] idx1
    // Имитируем исчезновение файла A мимо нашего scrub: сносим буфер + reject чтения с диска.
    useWorkspaceStore.setState((s) => ({
      buffers: Object.fromEntries(Object.entries(s.buffers).filter(([p]) => p !== 'A.md')),
    }));
    const spy = vi.spyOn(tauriApi.vault, 'readFileMeta').mockRejectedValueOnce(new Error('gone'));
    await ws().navBack();
    expect(ws().navIndex).toBe(1); // курсор не двинулся — openFile реджектнулся, поймали
    expect(activePath(ws())).toBe('B.md'); // активный документ прежний
    spy.mockRestore();
  });

  // NAV-4: позиция курсора запоминается на буфере (восстановление — в Editor/CM6).
  it('setBufferCursor запоминает позицию; не трогает dirty', async () => {
    await ws().openFile('README.md');
    ws().setBufferCursor('README.md', 42);
    expect(ws().buffers['README.md'].cursor).toBe(42);
    expect(ws().buffers['README.md'].dirty).toBe(false); // не правка контента
  });

  it('setBufferCursor по несуществующему пути — no-op', () => {
    ws().setBufferCursor('Ghost.md', 5);
    expect(ws().buffers['Ghost.md']).toBeUndefined();
  });

  it('rename переносит курсор вместе с буфером (NAV-4 живёт с буфером)', async () => {
    await ws().openFile('Old.md');
    ws().setBufferCursor('Old.md', 17);
    ws().renameBufferPath('Old.md', 'New.md');
    expect(ws().buffers['New.md'].cursor).toBe(17);
  });
});

// audit B8: reset (смена vault) обязан обнулить недавние и в localStorage, а не только в памяти —
// иначе следующий запуск/новый vault показывает recents прошлого. localStorage под node 25 в jsdom
// нерабочий, поэтому мокаем in-memory (как chat.test.ts).
describe('workspace reset чистит recents и в localStorage (audit B8)', () => {
  const RECENTS_KEY = 'nexus.recents.v1';
  beforeEach(async () => {
    const ls = new Map<string, string>();
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => (ls.has(k) ? (ls.get(k) as string) : null),
      setItem: (k: string, v: string) => void ls.set(k, String(v)),
      removeItem: (k: string) => void ls.delete(k),
      clear: () => ls.clear(),
    });
    useWorkspaceStore.getState().reset();
    useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
    await useVaultStore.getState().openVault('');
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('после reset recents пусты и в памяти, и в localStorage', async () => {
    await useWorkspaceStore.getState().openFile('A.md');
    await useWorkspaceStore.getState().openFile('B.md');
    expect(useWorkspaceStore.getState().recents.length).toBeGreaterThan(0);
    expect(JSON.parse(localStorage.getItem(RECENTS_KEY) ?? '[]').length).toBeGreaterThan(0);

    useWorkspaceStore.getState().reset();
    expect(useWorkspaceStore.getState().recents).toEqual([]);
    expect(JSON.parse(localStorage.getItem(RECENTS_KEY) ?? 'null')).toEqual([]);
  });
});
