import { StrictMode } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ChatView } from './ChatView';
import { tauriApi } from '../../lib/tauri-api';
import { disclosureOpen, useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import { useWorkspaceStore } from '../../stores/workspace';
import { useToastStore } from '../../stores/toast';
import * as activeView from '../../lib/editor/activeView';

beforeEach(() => {
  useChatStore.setState({ messages: [], streaming: false, mode: 'vault', web: false, pinned: [] });
  useToastStore.setState({ toasts: [] });
  disclosureOpen.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
  useWorkspaceStore.getState().reset(); // без утечки активного файла между тестами
});

describe('ChatView (Ф1-8)', () => {
  // Ревизия владельца 11.06 (вторая итерация): Web — флаг ПОВЕРХ режима, сегмент НЕ трогает.
  it('Web — независимый тоггл: режим не сбрасывается, сегмент остаётся активным', () => {
    render(<ChatView />);
    expect(screen.getAllByRole('radio')).toHaveLength(2);

    fireEvent.click(screen.getByRole('radio', { name: /общий|general/i }));
    expect(useChatStore.getState().mode).toBe('general');

    fireEvent.click(screen.getByRole('button', { name: /web/i, pressed: false }));
    expect(useChatStore.getState().web).toBe(true);
    expect(useChatStore.getState().mode).toBe('general');
    // Сегмент живой: можно сменить режим при включённом Web.
    const vaultRadio = screen.getByRole('radio', { name: /по заметкам|notes/i });
    expect(vaultRadio).not.toBeDisabled();
    fireEvent.click(vaultRadio);
    expect(useChatStore.getState().mode).toBe('vault');
    expect(useChatStore.getState().web).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: /web/i, pressed: true }));
    expect(useChatStore.getState().web).toBe(false);
    expect(useChatStore.getState().mode).toBe('vault');
  });

  // audit B10: radiogroup-режим — roving tabindex + стрелки переключают (WAI-ARIA radio pattern).
  it('radiogroup: стрелки переключают режим, roving tabindex (a11y, audit B10)', () => {
    render(<ChatView />);
    const group = screen.getByRole('radiogroup');
    const vaultRadio = screen.getByRole('radio', { name: /по заметкам|notes/i });
    const generalRadio = screen.getByRole('radio', { name: /общий|general/i });
    // выбранный режим (vault по умолчанию) — в табовом порядке, остальные изъяты.
    expect(vaultRadio).toHaveAttribute('tabindex', '0');
    expect(generalRadio).toHaveAttribute('tabindex', '-1');

    fireEvent.keyDown(group, { key: 'ArrowRight' });
    expect(useChatStore.getState().mode).toBe('general');
    expect(generalRadio).toHaveAttribute('tabindex', '0');
    expect(vaultRadio).toHaveAttribute('tabindex', '-1');

    fireEvent.keyDown(group, { key: 'ArrowLeft' });
    expect(useChatStore.getState().mode).toBe('vault');
  });

  it('пустое состояние — подсказка', () => {
    render(<ChatView />);
    expect(screen.getByText(/Спросите что-нибудь о ваших заметках/)).toBeInTheDocument();
  });

  it('рендерит ответ с источником; клик открывает файл', () => {
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'Где план?' },
        {
          id: 'a1',
          role: 'assistant',
          content: 'План здесь [1]',
          sources: [
            {
              chunkId: 7,
              path: 'Projects/Roadmap.md',
              title: null,
              headingPath: null,
              snippet: 'план…',
              score: 0.5,
            },
          ],
        },
      ],
      streaming: false,
    });
    const openFile = vi.fn(() => Promise.resolve());
    useWorkspaceStore.setState({ openFile });

    render(<ChatView />);
    // AIP-2: [1] в ответе — кликабельная цитата, открывает источник 1.
    fireEvent.click(screen.getByRole('button', { name: /Открыть источник|Open source/ }));
    expect(openFile).toHaveBeenCalledWith('Projects/Roadmap.md');
    // Источники также доступны компактной плашкой (Sonnet-style) — раскрываем и кликаем карточку.
    fireEvent.click(screen.getByRole('button', { name: /Источники · 1|Sources · 1/ }));
    fireEvent.click(screen.getByText('Roadmap')); // DP-15/DP-12: источник — title без .md
    expect(openFile).toHaveBeenCalledWith('Projects/Roadmap.md');
  });

  it('AIP-2: цитата [1] на web-источник открывает URL системным браузером', () => {
    const open = vi.spyOn(tauriApi.external, 'open').mockResolvedValue(undefined);
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'Что нового?' },
        {
          id: 'a1',
          role: 'assistant',
          content: 'Согласно источнику [1] всё ок',
          webSources: [{ title: 'Хабр', url: 'https://habr.com/x', snippet: 's' }],
        },
      ],
      streaming: false,
    });
    render(<ChatView />);
    fireEvent.click(screen.getByRole('button', { name: /Открыть источник|Open source/ }));
    expect(open).toHaveBeenCalledWith('https://habr.com/x');
  });

  // Регресс на находку adversarial-ревью: ответ ИИ с GFM-чеклистом + цитатой — `[ ]` остаётся
  // чекбоксом (remark-gfm раньше), а `[1]` становится цитатой (порядок плагинов не ломает таск-лист).
  it('AIP-2: чеклист `- [ ] …` + цитата [1] не конфликтуют', () => {
    const openFile = vi.fn(() => Promise.resolve());
    useWorkspaceStore.setState({ openFile });
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'План?' },
        {
          id: 'a1',
          role: 'assistant',
          content: '- [ ] купить молоко [1]',
          sources: [
            { chunkId: 1, path: 'Plan.md', title: null, headingPath: null, snippet: 's', score: 0.5 },
          ],
        },
      ],
      streaming: false,
    });
    render(<ChatView />);
    expect(screen.getByRole('checkbox')).toBeInTheDocument(); // чеклист цел
    fireEvent.click(screen.getByRole('button', { name: /Открыть источник|Open source/ }));
    expect(openFile).toHaveBeenCalledWith('Plan.md'); // цитата работает
  });

  it('AIP-2: цитата с номером вне диапазона источников — обычный текст, не кнопка', () => {
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'Где?' },
        {
          id: 'a1',
          role: 'assistant',
          content: 'Видно в [9]',
          sources: [
            { chunkId: 1, path: 'A.md', title: null, headingPath: null, snippet: 's', score: 0.5 },
          ],
        },
      ],
      streaming: false,
    });
    render(<ChatView />);
    expect(screen.queryByRole('button', { name: /Открыть источник|Open source/ })).toBeNull();
    expect(screen.getByText(/Видно в/)).toBeInTheDocument();
  });

  it('AIP-3: draft предзаполняет композер и сбрасывается (мост «Разобрать с ИИ»)', () => {
    useChatStore.setState({ draft: 'Разбери этот вопрос' });
    render(<ChatView />);
    const ta = screen.getByPlaceholderText(/Спросите о заметках/) as HTMLTextAreaElement;
    expect(ta.value).toBe('Разбери этот вопрос');
    expect(useChatStore.getState().draft).toBe(''); // потреблён один раз
  });

  it('AIP-3: под StrictMode draft потребляется ОДИН раз (ref-гард, не дублируется)', () => {
    // StrictMode дважды прогоняет mount-эффект с тем же draft ДО того, как setDraft('') долетит до
    // стора → без ref-гарда prefill дописался бы дважды («инсайт\nинсайт»).
    useChatStore.setState({ draft: 'инсайт' });
    render(
      <StrictMode>
        <ChatView />
      </StrictMode>,
    );
    const ta = screen.getByPlaceholderText(/Спросите о заметках/) as HTMLTextAreaElement;
    expect(ta.value).toBe('инсайт');
  });

  it('AIP-3: draft НЕ затирает уже набранный текст (дописывает с новой строки)', () => {
    render(<ChatView />);
    const ta = screen.getByPlaceholderText(/Спросите о заметках/) as HTMLTextAreaElement;
    fireEvent.change(ta, { target: { value: 'мой черновик' } });
    act(() => useChatStore.getState().setDraft('вопрос инсайта'));
    expect(ta.value).toBe('мой черновик\nвопрос инсайта'); // данные пользователя не потеряны
  });

  it('Enter отправляет вопрос', async () => {
    render(<ChatView />);
    const ta = screen.getByPlaceholderText(/Спросите о заметках/);
    fireEvent.change(ta, { target: { value: 'Roadmap' } });
    fireEvent.keyDown(ta, { key: 'Enter' });
    expect(await screen.findByText('Roadmap')).toBeInTheDocument();
    useChatStore.getState().stop();
  });

  it('кнопка отправки заблокирована при пустом вводе', () => {
    render(<ChatView />);
    expect(screen.getByRole('button', { name: 'Отправить' })).toBeDisabled();
  });

  it('пустое состояние: заголовок + suggestion-pill отправляет вопрос', () => {
    vi.spyOn(tauriApi.chat, 'streamRag').mockReturnValue(() => {}); // без асинхронного стрима
    render(<ChatView />);
    expect(screen.getByText('Спросите свои заметки')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Как устроен RAG в Qasr?' }));
    // отправка → появилось пользовательское сообщение с текстом пилюли (пустое состояние ушло)
    expect(screen.getByText('Как устроен RAG в Qasr?')).toBeInTheDocument();
    useChatStore.getState().stop();
  });

  it('reasoning (R1): живая сводка стримится в label «думает» (brand-mark, без плашки/спойлера)', () => {
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'Сколько будет 2+2*2?' },
        {
          id: 'a1',
          role: 'assistant',
          content: '',
          streaming: true,
          reasoningSummary: 'Сначала умножаю, потом складываю',
        },
      ],
      streaming: true,
    });
    const { container } = render(<ChatView />);
    // Сводка показана как переливающийся label фазы «думает».
    expect(screen.getByText('Сначала умножаю, потом складываю')).toBeInTheDocument();
    // Анимированный brand-mark присутствует (SVG-созвездие).
    expect(container.querySelector('svg')).toBeInTheDocument();
    // Старой уродливой плашки/спойлера «Ход рассуждений» больше нет.
    expect(screen.queryByText('Ход рассуждений')).not.toBeInTheDocument();
    expect(container.querySelector('details')).not.toBeInTheDocument();
  });

  it('reasoning (R1): до первой сводки label «думает» = дефолтная фраза', () => {
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'Привет' },
        { id: 'a1', role: 'assistant', content: '', streaming: true },
      ],
      streaming: true,
    });
    const { container } = render(<ChatView />);
    // «Ищу по заметкам…» показывается и в label «думает», и в пульсе композера — оба штатны.
    expect(screen.getAllByText(/Ищу по заметкам/).length).toBeGreaterThanOrEqual(1);
    expect(container.querySelector('svg')).toBeInTheDocument(); // brand-mark на месте
  });

  it('AC-EGR-14: типизированный отказ эгресса → i18n-баннер, не сырая строка', () => {
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'Вопрос' },
        {
          id: 'a1',
          role: 'assistant',
          content: '',
          error: 'egress: офлайн-режим …сырая строка…',
          deniedKind: 'offline',
        },
      ],
      streaming: false,
    });
    render(<ChatView />);
    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('Офлайн-режим включён')).toBeInTheDocument();
    // Сырой текст ошибки в баннере не показывается.
    expect(screen.queryByText(/сырая строка/)).not.toBeInTheDocument();
  });

  // P6-AR: действия под ответом ИИ.
  const assistantMsg = (content: string) => [
    { id: 'u1' as const, role: 'user' as const, content: 'q' },
    { id: 'a1' as const, role: 'assistant' as const, content },
  ];

  it('P6-AR: под готовым ответом есть «Копировать» и «Вставить в заметку»', () => {
    useChatStore.setState({ messages: assistantMsg('Ответ ИИ'), streaming: false });
    render(<ChatView />);
    expect(screen.getByRole('button', { name: 'Копировать' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Вставить в заметку' })).toBeInTheDocument();
  });

  it('P6-AR: «Копировать» кладёт ответ в буфер обмена + тост', async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    useChatStore.setState({ messages: assistantMsg('Текст ответа'), streaming: false });
    render(<ChatView />);
    fireEvent.click(screen.getByRole('button', { name: 'Копировать' }));
    expect(writeText).toHaveBeenCalledWith('Текст ответа');
    await vi.waitFor(() =>
      expect(useToastStore.getState().toasts.some((t) => t.message === 'Ответ скопирован')).toBe(true),
    );
  });

  it('P6-AR: «Вставить» без открытой заметки (Home/News) — «откройте заметку»', () => {
    vi.spyOn(activeView, 'getActiveEditorView').mockReturnValue(null);
    useWorkspaceStore.getState().reset(); // нет активной вкладки
    useChatStore.setState({ messages: assistantMsg('Текст'), streaming: false });
    render(<ChatView />);
    fireEvent.click(screen.getByRole('button', { name: 'Вставить в заметку' }));
    expect(
      useToastStore.getState().toasts.some((t) => t.message === 'Откройте заметку для вставки'),
    ).toBe(true);
  });

  // AIP-11: при открытой заметке в режиме vault — чипы-кандидаты в контекст (suggest.related), клик пинит.
  it('AIP-11: связанные с открытой заметкой → чипы в контекст; клик пинит', async () => {
    useWorkspaceStore.setState({
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
    });
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([
      { path: 'Notes/Related.md', title: 'Related', score: 0.9, reason: 'про то же' },
    ]);
    render(<ChatView />);

    expect(await screen.findByText(/в контекст|add to context/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Related/ }));
    expect(useChatStore.getState().pinned).toContain('Notes/Related.md');
    // Запинено → больше не предлагается (ряд кандидатов исчезает).
    await vi.waitFor(() =>
      expect(screen.queryByText(/в контекст|add to context/i)).not.toBeInTheDocument(),
    );
  });

  // AIP-11: в режиме «общий» (не vault) саджесты не запрашиваются и не показываются.
  it('AIP-11: режим general → саджесты не показываются', async () => {
    useWorkspaceStore.setState({
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
    });
    useChatStore.setState({ mode: 'general' });
    const related = vi.spyOn(tauriApi.suggest, 'related');
    render(<ChatView />);
    expect(screen.queryByText(/в контекст|add to context/i)).not.toBeInTheDocument();
    expect(related).not.toHaveBeenCalled();
  });

  // AIP-11 degrade: нет векторов/индекса (related реджектится) → ряд скрыт, без падения.
  it('AIP-11: ошибка related → ряд кандидатов скрыт (без краша)', async () => {
    useWorkspaceStore.setState({
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
    });
    const related = vi.spyOn(tauriApi.suggest, 'related').mockRejectedValue(new Error('no vectors'));
    render(<ChatView />);
    await vi.waitFor(() => expect(related).toHaveBeenCalled());
    expect(screen.queryByText(/в контекст|add to context/i)).not.toBeInTheDocument();
  });

  // AIP-11: активный файл (его пинит «Прикрепить») и уже-закреплённые исключены из кандидатов.
  it('AIP-11: сам активный файл и закреплённые исключены из кандидатов', async () => {
    useWorkspaceStore.setState({
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
    });
    useChatStore.setState({ pinned: ['Notes/Pinned.md'] });
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([
      { path: 'A.md', title: 'A', score: 0.99, reason: 'self' }, // = активный файл → исключён
      { path: 'Notes/Pinned.md', title: 'Pinned', score: 0.9, reason: 'r' }, // уже закреплён → исключён
      { path: 'Notes/Fresh.md', title: 'Fresh', score: 0.8, reason: 'r' }, // только этот — кандидат
    ]);
    const { container } = render(<ChatView />);

    expect(await screen.findByRole('button', { name: /Fresh/ })).toBeInTheDocument();
    // среди кандидат-чипов (suggestChip) ровно один — Fresh; A (=активный) и Pinned (закреплён) отфильтрованы.
    const candidateChips = Array.from(container.querySelectorAll('[class*="suggestChip"]'));
    expect(candidateChips).toHaveLength(1);
    expect(candidateChips[0].textContent).toContain('Fresh');
  });

  it('P6-AR: «Вставить» в режиме чтения (заметка открыта, но CM6 размонтирован) — честная подсказка', () => {
    vi.spyOn(activeView, 'getActiveEditorView').mockReturnValue(null);
    // Заметка открыта (активная вкладка есть), но редактора нет (preview/reading).
    useWorkspaceStore.setState({
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
    });
    useChatStore.setState({ messages: assistantMsg('Текст'), streaming: false });
    render(<ChatView />);
    fireEvent.click(screen.getByRole('button', { name: 'Вставить в заметку' }));
    expect(
      useToastStore.getState().toasts.some((t) => t.message === 'Переключитесь в режим редактирования'),
    ).toBe(true);
  });

  it('P6-AR: «Вставить» с активным редактором — dispatch вставки у курсора + тост', () => {
    const dispatch = vi.fn();
    const focus = vi.fn();
    const fakeView = {
      state: { selection: { main: { from: 3, to: 3 } } },
      dispatch,
      focus,
    } as unknown as ReturnType<typeof activeView.getActiveEditorView>;
    vi.spyOn(activeView, 'getActiveEditorView').mockReturnValue(fakeView);
    useChatStore.setState({ messages: assistantMsg('ВСТАВКА'), streaming: false });
    render(<ChatView />);
    fireEvent.click(screen.getByRole('button', { name: 'Вставить в заметку' }));
    expect(dispatch).toHaveBeenCalledWith({
      changes: { from: 3, to: 3, insert: 'ВСТАВКА' },
      selection: { anchor: 3 + 'ВСТАВКА'.length },
    });
    expect(useToastStore.getState().toasts.some((t) => t.message === 'Вставлено в заметку')).toBe(true);
  });

  it('P6-RGN: «Перегенерировать» есть у ПОСЛЕДНЕГО ответа, нет у предыдущих', async () => {
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'q1' },
        { id: 'a1', role: 'assistant', content: 'старый ответ' },
        { id: 'u2', role: 'user', content: 'q2' },
        { id: 'a2', role: 'assistant', content: 'последний ответ' },
      ],
      streaming: false,
    });
    render(<ChatView />);
    // одна кнопка регенерации — у последнего ответа.
    expect(screen.getAllByRole('button', { name: 'Перегенерировать' })).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Перегенерировать' }));
    // последний обмен убран, переспрошен q2 (вопрос на месте, ответ новый/стримится) — асинхронно.
    await vi.waitFor(() => {
      const msgs = useChatStore.getState().messages;
      expect(msgs[msgs.length - 2]).toMatchObject({ role: 'user', content: 'q2' });
    });
    useChatStore.getState().stop();
  });

  it('P6-AR: под стримящимся ответом действий нет', () => {
    useChatStore.setState({
      messages: [
        { id: 'u1', role: 'user', content: 'q' },
        { id: 'a1', role: 'assistant', content: 'частичный…', streaming: true },
      ],
      streaming: true,
    });
    render(<ChatView />);
    expect(screen.queryByRole('button', { name: 'Копировать' })).not.toBeInTheDocument();
  });

  // P6-PIN: чипы закреплённых заметок над композером.
  it('P6-PIN: закреплённые заметки показаны чипами; × открепляет', () => {
    useChatStore.setState({ pinned: ['Notes/Idea.md'], streaming: false });
    render(<ChatView />);
    expect(screen.getByText('Idea')).toBeInTheDocument(); // имя без папки/.md
    fireEvent.click(screen.getByRole('button', { name: 'Открепить' }));
    expect(useChatStore.getState().pinned).toEqual([]);
  });

  it('DP-12: стиль источников chips/footnotes переключается настройкой ragSources', () => {
    const msgs = [
      { id: 'u1' as const, role: 'user' as const, content: 'Где план?' },
      {
        id: 'a1' as const,
        role: 'assistant' as const,
        content: 'Тут',
        sources: [
          {
            chunkId: 7,
            path: 'Projects/Roadmap.md',
            title: null,
            headingPath: null,
            snippet: 'план…',
            score: 0.5,
          },
        ],
      },
    ];
    useChatStore.setState({ messages: msgs, streaming: false });
    usePrefsStore.setState({ ragSources: 'chips' });
    const { unmount } = render(<ChatView />);
    fireEvent.click(screen.getByRole('button', { name: /Источники · 1|Sources · 1/ }));
    expect(screen.getByRole('button', { name: /1.*Roadmap/ })).toBeInTheDocument();
    unmount();

    usePrefsStore.setState({ ragSources: 'footnotes' });
    render(<ChatView />);
    // Раскрытость пережила перемонтирование (реестр вне React — фикс «сворачивались при скролле»
    // из-за размонтирования виртуализацией): повторный клик не нужен.
    expect(screen.getByRole('button', { name: /Источники · 1|Sources · 1/ })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    expect(screen.getByText('[1]')).toBeInTheDocument(); // сноска `[N]`
    usePrefsStore.setState({ ragSources: 'cards' });
  });
});
