import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GroupPane } from './GroupPane';
import { tauriApi } from '../../lib/tauri-api';
import { usePrefsStore } from '../../stores/prefs';
import { useWorkspaceStore } from '../../stores/workspace';

// Пустая группа (без вкладок): таб-стрип с back/forward рендерится всегда, без CM6-редактора —
// изолируем проверку nav-кнопок от ленивого превью/Editor.
function setupNav(navHistory: { path: string; groupId: string }[], navIndex: number) {
  useWorkspaceStore.setState({
    groups: [{ id: 'g0', tabs: [], activeTab: null }],
    activeGroupId: 'g0',
    buffers: {},
    navHistory,
    navIndex,
  });
}

beforeEach(() => useWorkspaceStore.getState().reset());
afterEach(() => vi.restoreAllMocks());

describe('GroupPane back/forward (NAV-3 кнопки)', () => {
  it('пустая история → обе кнопки disabled', () => {
    setupNav([], -1);
    render(<GroupPane groupId="g0" />);
    expect(screen.getByRole('button', { name: 'Назад' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Вперёд' })).toBeDisabled();
  });

  it('на левом крае истории: Назад disabled, Вперёд активна', () => {
    setupNav([{ path: 'A.md', groupId: 'g0' }, { path: 'B.md', groupId: 'g0' }], 0);
    render(<GroupPane groupId="g0" />);
    expect(screen.getByRole('button', { name: 'Назад' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Вперёд' })).toBeEnabled();
  });

  it('на правом крае истории: Назад активна, Вперёд disabled', () => {
    setupNav([{ path: 'A.md', groupId: 'g0' }, { path: 'B.md', groupId: 'g0' }], 1);
    render(<GroupPane groupId="g0" />);
    expect(screen.getByRole('button', { name: 'Назад' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Вперёд' })).toBeDisabled();
  });

  it('клик «Назад» зовёт существующий navBack стора (логика не дублируется)', () => {
    setupNav([{ path: 'A.md', groupId: 'g0' }, { path: 'B.md', groupId: 'g0' }], 1);
    const navBack = vi.spyOn(useWorkspaceStore.getState(), 'navBack').mockResolvedValue();
    render(<GroupPane groupId="g0" />);
    fireEvent.click(screen.getByRole('button', { name: 'Назад' }));
    expect(navBack).toHaveBeenCalledTimes(1);
  });

  it('клик «Вперёд» зовёт существующий navForward стора', () => {
    setupNav([{ path: 'A.md', groupId: 'g0' }, { path: 'B.md', groupId: 'g0' }], 0);
    const navForward = vi.spyOn(useWorkspaceStore.getState(), 'navForward').mockResolvedValue();
    render(<GroupPane groupId="g0" />);
    fireEvent.click(screen.getByRole('button', { name: 'Вперёд' }));
    expect(navForward).toHaveBeenCalledTimes(1);
  });
});

// W-1: крестик закрытия пейна — появляется ТОЛЬКО при сплите (>1 группы), зовёт closeGroup.
describe('GroupPane close-pane (W-1)', () => {
  it('одна группа → кнопки «Закрыть панель» нет (последний пейн не закрыть)', () => {
    useWorkspaceStore.setState({
      groups: [{ id: 'g0', tabs: [], activeTab: null }],
      activeGroupId: 'g0',
      buffers: {},
    });
    render(<GroupPane groupId="g0" />);
    expect(screen.queryByRole('button', { name: 'Закрыть панель' })).toBeNull();
  });

  it('две группы → кнопка есть и зовёт closeGroup для своего пейна', () => {
    useWorkspaceStore.setState({
      groups: [
        { id: 'g0', tabs: [], activeTab: null },
        { id: 'g1', tabs: [], activeTab: null },
      ],
      activeGroupId: 'g1',
      buffers: {},
    });
    const closeGroup = vi
      .spyOn(useWorkspaceStore.getState(), 'closeGroup')
      .mockResolvedValue(undefined); // P0-5: closeGroup теперь async
    render(<GroupPane groupId="g1" />);
    fireEvent.click(screen.getByRole('button', { name: 'Закрыть панель' }));
    expect(closeGroup).toHaveBeenCalledWith('g1');
  });
});

// ── EDFIX-4 F4: mode-float пилюля ВНЕ скролл-контейнера (не уезжает при прокрутке) + fallback
//    режима из персист-префа noteMode (новая панель без записи в modes наследует последний выбранный). ──
describe('GroupPane mode-float + преф noteMode (EDFIX-4 F4)', () => {
  function setupMd(noteMode: 'source' | 'preview') {
    vi.spyOn(tauriApi.vault, 'fileMtime').mockResolvedValue(0);
    usePrefsStore.setState({ noteMode });
    useWorkspaceStore.setState({
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
      buffers: { 'A.md': { path: 'A.md', doc: '# T\n\nтекст', dirty: false, baseHash: '' } },
      modes: {}, // явной записи НЕТ — режим наследуется из префа
    });
    return render(<GroupPane groupId="g0" />);
  }
  afterEach(() => usePrefsStore.setState({ noteMode: 'source' }));

  it('пилюля НЕ внутри .scroll (absolute относительно editorCol → не скроллится с контентом)', () => {
    setupMd('source');
    const pill = screen.getByRole('button', { name: 'Просмотр' });
    expect(pill.closest('[class*="scroll"]')).toBeNull();
    expect(pill.closest('[class*="editorCol"]')).not.toBeNull();
  });

  it('панель без записи в modes наследует преф noteMode=preview (пилюля показывает «Исходник»)', async () => {
    setupMd('preview');
    // Режим preview → лениво грузится MarkdownPreview, пилюля предлагает обратное действие.
    expect(await screen.findByRole('button', { name: 'Исходник' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Исходник' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('панель без записи в modes при префе source стартует в source (пилюля «Просмотр»)', () => {
    setupMd('source');
    expect(screen.getByRole('button', { name: 'Просмотр' })).toHaveAttribute('aria-pressed', 'false');
  });
});

// S6-FIX2: прыжок оглавления к строке в СВЁРНУТОЙ секции должен ОТЛОЖИТЬ scrollIntoView до конца
// grid-анимации раскрытия (transitionend / фолбэк-таймер), а к УЖЕ развёрнутой — скроллить немедленно (rAF).
describe('GroupPane jumpToHeading expand-then-scroll (S6-FIX2)', () => {
  // Заметка: h2 «Раздел» (стр.1) + вложенный h3 «Под» (стр.3) + тело.
  const SRC = '## Раздел\n\n### Под\n\nтекст внутри';

  // jsdom не реализует scrollIntoView — ставим no-op стуб на прототип, чтобы его можно было шпионить.
  // vi.useRealTimers() в beforeEach/afterEach — защита от утечки fake timers из других файлов:
  // vitest запускает несколько файлов в одном воркере, и если предыдущий файл не отменил
  // vi.useFakeTimers(), screen.findByRole (RTL-поллер на реальном setInterval) виснет.
  beforeEach(() => {
    if (!HTMLElement.prototype.scrollIntoView) HTMLElement.prototype.scrollIntoView = () => {};
    vi.useRealTimers(); // сброс утечших fake timers
  });
  afterEach(() => {
    vi.useRealTimers(); // гарантия очистки, если тест кинул исключение до finally{}
  });

  // Открыть markdown-вкладку в режиме preview; дождаться ленивого MarkdownPreview.
  async function setup() {
    vi.spyOn(tauriApi.vault, 'fileMtime').mockResolvedValue(0);
    useWorkspaceStore.setState({
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
      buffers: { 'A.md': { path: 'A.md', doc: SRC, dirty: false, baseHash: '' } },
      modes: { g0: 'preview' },
    });
    render(<GroupPane groupId="g0" />);
    // Ленивый MarkdownPreview подгрузился — виден заголовок секции.
    await screen.findByRole('heading', { name: /Раздел/, level: 2 });
  }

  // Прокинуть jumpToHeading: открыть инспектор «Оглавление» и кликнуть пункт нужной строки.
  function clickOutline(name: RegExp) {
    fireEvent.click(screen.getByRole('button', { name: 'Оглавление' }));
    fireEvent.click(screen.getByRole('button', { name }));
  }

  it('цель в свёрнутой секции → scrollIntoView ОТЛОЖЕН до transitionend(grid-template-rows)', async () => {
    await setup();
    const scrollSpy = vi.fn();
    vi.spyOn(HTMLElement.prototype, 'scrollIntoView').mockImplementation(scrollSpy);

    // ANTI-FLAKE 2-й заход (CI 2026-07-07): новый режим отказа, иной чем #499.
    // Суть: React 19 шедулит setState через MessageChannel (macrotask). Под нагрузкой
    // (3× parallel CI) MessageChannel callback задерживается; act(async) и vi.waitFor
    // ненадёжны под fake timers: MessageChannel не перехватывается заменой setTimeout.
    //
    // Фикс: сворачивание выполняется с РЕАЛЬНЫМИ таймерами; ждём коммита через
    // screen.findByRole (RTL-поллер: реальный setInterval, timeout 1000мс, надёжен
    // под любой нагрузкой). ПОСЛЕ того как DOM содержит «Развернуть секцию» (=collapsed),
    // переключаемся на fake timers — они управляют только jumpToHeading (rAF + 350мс-фолбэк).
    fireEvent.click(screen.getByRole('heading', { name: /Раздел/, level: 2 }));
    // «Развернуть секцию» — aria-label кнопки шеврона при collapsed=true.
    await screen.findByRole('button', { name: 'Развернуть секцию' });
    vi.useFakeTimers();
    try {
      // Прыжок к вложенному h3 «Под» (строка 3, скрыта в свёрнутом теле).
      clickOutline(/Под/);
      // Первый кадр (rAF = fake setTimeout(0)): scrollIntoView ещё НЕ вызван (ждём анимацию раскрытия).
      act(() => {
        vi.advanceTimersByTime(20);
      });
      expect(scrollSpy).not.toHaveBeenCalled();
      // Эмулируем завершение grid-анимации → теперь скроллит.
      const secBody = document.querySelector(
        'section[data-sec-id="раздел"] .sec-body',
      ) as HTMLElement;
      expect(secBody).not.toBeNull();
      act(() => {
        const ev = new Event('transitionend') as TransitionEvent;
        Object.defineProperty(ev, 'propertyName', { value: 'grid-template-rows' });
        secBody.dispatchEvent(ev);
      });
      expect(scrollSpy).toHaveBeenCalledTimes(1);
      // 350мс-фолбэк после состоявшегося transitionend НЕ даёт второго скролла.
      act(() => {
        vi.advanceTimersByTime(400);
      });
      expect(scrollSpy).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('цель в РАЗВЁРНУТОЙ секции → scrollIntoView немедленный (в rAF, без ожидания transitionend)', async () => {
    await setup();
    const scrollSpy = vi.fn();
    vi.spyOn(HTMLElement.prototype, 'scrollIntoView').mockImplementation(scrollSpy);
    // Секция развёрнута по умолчанию → прыжок к h3 «Под» (видим) скроллит сразу в rAF.
    clickOutline(/Под/);
    await act(async () => {
      await new Promise((r) => requestAnimationFrame(() => r(null)));
    });
    expect(scrollSpy).toHaveBeenCalledTimes(1);
  });

  it('фолбэк-таймер: если transitionend не пришёл, scrollIntoView всё равно срабатывает (~350мс)', async () => {
    await setup();
    const scrollSpy = vi.fn();
    vi.spyOn(HTMLElement.prototype, 'scrollIntoView').mockImplementation(scrollSpy);

    // Сворачиваем с реальными таймерами; ждём коммита через findByRole (надёжно под нагрузкой).
    fireEvent.click(screen.getByRole('heading', { name: /Раздел/, level: 2 })); // свернуть
    await screen.findByRole('button', { name: 'Развернуть секцию' });
    vi.useFakeTimers();
    try {
      clickOutline(/Под/);
      // Прогнать rAF + фолбэк-таймер (transitionend НЕ диспатчим).
      act(() => {
        vi.advanceTimersByTime(400);
      });
      expect(scrollSpy).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
