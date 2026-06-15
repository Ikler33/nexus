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
});
