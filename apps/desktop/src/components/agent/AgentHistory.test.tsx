import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { AgentHistory } from './AgentHistory';
import { useAgentStore } from '../../stores/agent';

/** Сброс стора агента между тестами (мок-история — детерминированные фейковые сессии). */
function reset() {
  useAgentStore.setState({
    turns: [],
    currentSessionId: 'sess-x',
    autonomy: 'confirm',
    model: 'qwen3:35b',
    perms: { read: true, write: true, web: false },
    context: null,
    approving: false,
  });
}

beforeEach(reset);
afterEach(() => {
  vi.restoreAllMocks();
});

describe('AgentHistory (W-38 — левый сайдбар истории переписок)', () => {
  it('рендерит сессии из мока (свежие сверху) + кнопку «Новая переписка»', async () => {
    render(<AgentHistory />);
    // Заголовок и кнопка новой переписки.
    expect(screen.getByRole('button', { name: 'Новая переписка' })).toBeInTheDocument();
    // Мок-сессии подгружаются асинхронно.
    expect(await screen.findByText('Разобрать входящие заметки')).toBeInTheDocument();
    expect(screen.getByText('Связать проекты RMS-B2B')).toBeInTheDocument();
    expect(screen.getByText('Сводка по PaymentService')).toBeInTheDocument();
  });

  it('клик по строке вызывает loadSession (переоткрытие переписки)', async () => {
    const spy = vi.spyOn(useAgentStore.getState(), 'loadSession');
    render(<AgentHistory />);
    const row = await screen.findByTitle('Разобрать входящие заметки');
    fireEvent.click(row);
    await waitFor(() => expect(spy).toHaveBeenCalledWith('sess-demo-1'));
  });

  it('активная строка = currentSessionId (aria-current)', async () => {
    useAgentStore.setState({ currentSessionId: 'sess-demo-2' });
    render(<AgentHistory />);
    const active = await screen.findByTitle('Связать проекты RMS-B2B');
    expect(active).toHaveAttribute('aria-current', 'true');
  });

  it('collapse-тоггл прячет бар (остаётся только ре-открыватель)', async () => {
    render(<AgentHistory />);
    await screen.findByText('Разобрать входящие заметки');
    // Сворачиваем.
    fireEvent.click(screen.getByRole('button', { name: 'Свернуть историю' }));
    // Список скрыт, виден только ре-открыватель.
    expect(screen.queryByText('Разобрать входящие заметки')).not.toBeInTheDocument();
    const reopen = screen.getByRole('button', { name: 'Показать историю' });
    expect(reopen).toBeInTheDocument();
    // Разворачиваем обратно.
    fireEvent.click(reopen);
    expect(await screen.findByText('Разобрать входящие заметки')).toBeInTheDocument();
  });
});
