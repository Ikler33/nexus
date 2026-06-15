import { fireEvent, render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { AiPanel } from './AiPanel';
import { usePrefsStore } from '../../stores/prefs';

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
