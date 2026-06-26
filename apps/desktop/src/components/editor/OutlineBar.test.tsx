import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { OutlineBar } from './OutlineBar';

const DOC = ['# Intro', '## Details', '### Deeper', 'body'].join('\n');

describe('OutlineBar (EDIT-7)', () => {
  it('рендерит заголовки списком; клик вызывает onJump с 1-based строкой', () => {
    const onJump = vi.fn();
    render(<OutlineBar doc={DOC} onJump={onJump} />);
    expect(screen.getByRole('button', { name: 'Intro' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Details' }));
    expect(onJump).toHaveBeenCalledWith(2); // `## Details` — строка 2
  });

  it('нет заголовков → ничего не рендерит (не шумит на коротких заметках)', () => {
    const { container } = render(<OutlineBar doc="просто текст без заголовков" onJump={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('шапка-твист сворачивает список', () => {
    render(<OutlineBar doc={DOC} onJump={() => {}} />);
    expect(screen.getByRole('button', { name: 'Intro' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { expanded: true })); // твист
    expect(screen.queryByRole('button', { name: 'Intro' })).toBeNull();
  });

  // audit B11: парсинг заголовков отложен (useDeferredValue) — корректность сохраняется: смена doc
  // в итоге отражается в оглавлении (deferred-значение дофлушивается).
  it('обновление doc отражается в оглавлении (отложенный парсинг корректен)', async () => {
    const { rerender } = render(<OutlineBar doc={DOC} onJump={() => {}} />);
    expect(screen.getByRole('button', { name: 'Intro' })).toBeInTheDocument();
    rerender(<OutlineBar doc={'# Renamed\n## Other'} onJump={() => {}} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Renamed' })).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Intro' })).toBeNull();
  });

  // Hermes-8 S6 scroll-spy: проп activeLine подсвечивает пункт, чья исходная строка совпадает.
  it('S6: activeLine подсвечивает соответствующий пункт (active-класс + aria-current=location)', () => {
    render(<OutlineBar doc={DOC} onJump={() => {}} activeLine={2} />);
    // `## Details` — исходная строка 2 → активен; пункт несёт aria-current=location.
    const details = screen.getByRole('button', { name: 'Details' });
    expect(details).toHaveAttribute('aria-current', 'location');
    expect(details.className).toMatch(/active/);
    // Прочие пункты — без подсветки.
    const intro = screen.getByRole('button', { name: 'Intro' });
    expect(intro).not.toHaveAttribute('aria-current');
    expect(intro.className).not.toMatch(/active/);
  });

  it('S6: activeLine не задан/нет совпадения → ни один пункт не активен (поведение прежнее)', () => {
    const { rerender } = render(<OutlineBar doc={DOC} onJump={() => {}} />);
    expect(screen.queryByRole('button', { current: 'location' })).toBeNull();
    // строка без заголовка → совпадения нет, подсветки нет
    rerender(<OutlineBar doc={DOC} onJump={() => {}} activeLine={99} />);
    expect(screen.queryByRole('button', { current: 'location' })).toBeNull();
    // null безвреден (как при пустом скролле над первым заголовком до initial-compute)
    rerender(<OutlineBar doc={DOC} onJump={() => {}} activeLine={null} />);
    expect(screen.queryByRole('button', { current: 'location' })).toBeNull();
  });
});
