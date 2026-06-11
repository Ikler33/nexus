import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ChatView } from './ChatView';
import { tauriApi } from '../../lib/tauri-api';
import { disclosureOpen, useChatStore } from '../../stores/chat';
import { usePrefsStore } from '../../stores/prefs';
import { useWorkspaceStore } from '../../stores/workspace';

beforeEach(() => {
  useChatStore.setState({ messages: [], streaming: false, mode: 'vault', web: false });
  disclosureOpen.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
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
    // Источники теперь свернуты компактной плашкой (Sonnet-style) — раскрываем.
    fireEvent.click(screen.getByRole('button', { name: /Источники · 1|Sources · 1/ }));
    expect(screen.getByText('План здесь [1]')).toBeInTheDocument();
    fireEvent.click(screen.getByText('Roadmap')); // DP-15/DP-12: источник — title без .md
    expect(openFile).toHaveBeenCalledWith('Projects/Roadmap.md');
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
    fireEvent.click(screen.getByRole('button', { name: 'Как устроен RAG в Nexus?' }));
    // отправка → появилось пользовательское сообщение с текстом пилюли (пустое состояние ушло)
    expect(screen.getByText('Как устроен RAG в Nexus?')).toBeInTheDocument();
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
