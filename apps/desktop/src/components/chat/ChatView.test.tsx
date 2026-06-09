import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ChatView } from './ChatView';
import { tauriApi } from '../../lib/tauri-api';
import { useChatStore } from '../../stores/chat';
import { useWorkspaceStore } from '../../stores/workspace';

beforeEach(() => {
  useChatStore.setState({ messages: [], streaming: false });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ChatView (Ф1-8)', () => {
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
    expect(screen.getByText('План здесь [1]')).toBeInTheDocument();
    fireEvent.click(screen.getByText('Projects/Roadmap.md'));
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
});
