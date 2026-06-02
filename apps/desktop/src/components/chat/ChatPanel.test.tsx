import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ChatPanel } from './ChatPanel';
import { useChatStore } from '../../stores/chat';
import { useWorkspaceStore } from '../../stores/workspace';

beforeEach(() => {
  useChatStore.setState({ messages: [], streaming: false });
});

describe('ChatPanel (Ф1-8)', () => {
  it('пустое состояние — подсказка', () => {
    render(<ChatPanel />);
    expect(screen.getByText(/Спросите что-нибудь о ваших заметках/)).toBeInTheDocument();
  });

  it('рендерит ответ с источником; клик по источнику открывает файл', () => {
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
              snippet: 'план проекта…',
              score: 0.5,
            },
          ],
        },
      ],
      streaming: false,
    });
    const openFile = vi.fn(() => Promise.resolve());
    useWorkspaceStore.setState({ openFile });

    render(<ChatPanel />);
    expect(screen.getByText('План здесь [1]')).toBeInTheDocument();

    fireEvent.click(screen.getByText('Projects/Roadmap.md'));
    expect(openFile).toHaveBeenCalledWith('Projects/Roadmap.md');
  });

  it('Enter отправляет вопрос (появляется сообщение пользователя)', async () => {
    render(<ChatPanel />);
    const ta = screen.getByPlaceholderText(/Спросите о заметках/);
    fireEvent.change(ta, { target: { value: 'Roadmap' } });
    fireEvent.keyDown(ta, { key: 'Enter' });

    expect(await screen.findByText('Roadmap')).toBeInTheDocument();
    useChatStore.getState().stop(); // прекращаем стрим, чтобы таймеры мока не текли в другие тесты
  });

  it('кнопка отправки заблокирована при пустом вводе', () => {
    render(<ChatPanel />);
    expect(screen.getByRole('button', { name: 'Отправить' })).toBeDisabled();
  });
});
