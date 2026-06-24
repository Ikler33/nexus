import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { AiPanel } from './AiPanel';
import { usePrefsStore } from '../../stores/prefs';
import { useChatStore } from '../../stores/chat';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AiPanel ресайз — очистка слушателей при размонтировании (audit B11)', () => {
  it('mousemove после unmount во время драга не дёргает setAiPanelW', () => {
    const spy = vi.spyOn(usePrefsStore.getState(), 'setAiPanelW');
    const { unmount, container } = render(<AiPanel variant="side" />);
    const resizer = container.querySelector('[role="separator"]') as HTMLElement;
    expect(resizer).not.toBeNull();

    fireEvent.mouseDown(resizer); // старт драга → mousemove/mouseup на window
    fireEvent.mouseMove(window, { clientX: 100 });
    expect(spy).toHaveBeenCalled(); // во время драга кромка двигает ширину
    spy.mockClear();

    unmount(); // размонтирование ВО ВРЕМЯ драга → AbortController снимает слушатели
    fireEvent.mouseMove(window, { clientX: 300 });
    expect(spy).not.toHaveBeenCalled(); // слушателей больше нет → нет stale-вызовов
  });
});

describe('AiPanel — поиск по переписке (#58, W-8)', () => {
  it('история → ввод запроса → совпадения со snippet; клик грузит сессию', async () => {
    const loadSession = vi.spyOn(useChatStore.getState(), 'loadSession').mockResolvedValue();
    render(<AiPanel variant="side" />);
    // Открыть дропдаун истории.
    fireEvent.click(screen.getByRole('button', { name: /История сессий|Session history/i }));
    const input = screen.getByLabelText(/Поиск по переписке|Search conversations/i);
    // Запрос совпадает с мок-сообщением сессии 1 («Как работает гибридный поиск?»).
    fireEvent.change(input, { target: { value: 'гибридный' } });
    const hit = await screen.findByText(/Гибридный поиск и RRF/);
    // Ревью: snippet-маркеры [..] рендерятся как <mark>, а не сырые скобки.
    const mark = document.querySelector('mark');
    expect(mark?.textContent?.toLowerCase()).toContain('гибридный');
    expect(document.body.textContent).not.toContain('[гибридный]');
    fireEvent.click(hit);
    expect(loadSession).toHaveBeenCalledWith(1);
  });

  it('пустых совпадений — «Совпадений нет»', async () => {
    render(<AiPanel variant="side" />);
    fireEvent.click(screen.getByRole('button', { name: /История сессий|Session history/i }));
    const input = screen.getByLabelText(/Поиск по переписке|Search conversations/i);
    fireEvent.change(input, { target: { value: 'zzzнеттакого' } });
    expect(await screen.findByText(/Совпадений нет|No matches/i)).toBeInTheDocument();
  });
});
