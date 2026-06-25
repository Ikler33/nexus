import { act, render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { StreamingMarkdown } from './StreamingMarkdown';

describe('StreamingMarkdown (W-34: троттл ~90мс + live-рендер)', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('первый кадр показывается сразу (initial text)', () => {
    const { container } = render(<StreamingMarkdown text={'**hi**'} />);
    expect(container.querySelector('strong')?.textContent).toBe('hi');
  });

  it('частая смена text коалесцируется: рендер обновляется не чаще ~90мс, финал показывается', () => {
    vi.useFakeTimers();
    const { container, rerender } = render(<StreamingMarkdown text={'a'} />);
    expect(container.textContent).toContain('a');

    // Поток токенов до истечения окна троттла — промежуточные кадры НЕ применяются.
    rerender(<StreamingMarkdown text={'ab'} />);
    rerender(<StreamingMarkdown text={'abc'} />);
    rerender(<StreamingMarkdown text={'abcd'} />);
    expect(container.textContent).toContain('a');
    expect(container.textContent).not.toContain('abcd');

    // По истечении окна — показывается ПОСЛЕДНЕЕ значение (коалесценция, не промежуточные).
    act(() => {
      vi.advanceTimersByTime(90);
    });
    expect(container.textContent).toContain('abcd');
  });

  it('недописанный код-блок рендерится как <pre><code> (не ломает разметку)', () => {
    const { container } = render(<StreamingMarkdown text={'```js\nconst x = 1;'} />);
    const pre = container.querySelector('pre');
    expect(pre).not.toBeNull();
    expect(pre?.querySelector('code')?.textContent).toContain('const x = 1;');
  });

  it('таймер чистится при размонтировании (без утечки/варнинга)', () => {
    vi.useFakeTimers();
    const clearSpy = vi.spyOn(globalThis, 'clearTimeout');
    const { rerender, unmount } = render(<StreamingMarkdown text={'a'} />);
    rerender(<StreamingMarkdown text={'ab'} />); // планирует таймер
    unmount();
    expect(clearSpy).toHaveBeenCalled();
  });
});
