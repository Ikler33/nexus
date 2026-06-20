import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { InlineAIBar } from './InlineAIBar';

const PLACEHOLDER = 'Спросите AI или опишите, что вставить…';

describe('InlineAIBar (InlineAI ⌘/ prompt-box)', () => {
  it('запрос → стрим (мок) → «Вставить» отдаёт результат в onInsert', async () => {
    const onInsert = vi.fn();
    render(<InlineAIBar note="Моя заметка" onInsert={onInsert} onClose={vi.fn()} />);

    const input = screen.getByPlaceholderText(PLACEHOLDER);
    fireEvent.change(input, { target: { value: 'список дел' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    // На фазе done появляется кнопка «Вставить» (мок стримит ответ с упоминанием запроса).
    const insertBtn = await screen.findByRole('button', { name: /Вставить/ }, { timeout: 3000 });
    fireEvent.click(insertBtn);
    expect(onInsert).toHaveBeenCalledTimes(1);
    expect(onInsert.mock.calls[0][0]).toContain('список дел');
  });

  it('Esc в инпуте закрывает (onClose)', () => {
    const onClose = vi.fn();
    render(<InlineAIBar note="" onInsert={vi.fn()} onClose={onClose} />);
    fireEvent.keyDown(screen.getByPlaceholderText(PLACEHOLDER), { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('пустой запрос — Enter не запускает стрим (остаёмся в фазе ask)', () => {
    render(<InlineAIBar note="" onInsert={vi.fn()} onClose={vi.fn()} />);
    fireEvent.keyDown(screen.getByPlaceholderText(PLACEHOLDER), { key: 'Enter' });
    expect(screen.getByPlaceholderText(PLACEHOLDER)).toBeInTheDocument();
    expect(screen.queryByText('Думаю…')).toBeNull();
  });
});
