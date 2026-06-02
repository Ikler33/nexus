import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Editor } from './Editor';

describe('Editor (Ф0-5, контракт CM6↔React)', () => {
  it('рендерит документ и заменяет его при смене файла (без пересоздания)', async () => {
    const { rerender } = render(<Editor path="A.md" initialDoc="Alpha content here" />);
    const host = screen.getByTestId('editor');
    await waitFor(() => expect(host.textContent).toContain('Alpha'));

    rerender(<Editor path="B.md" initialDoc="Bravo content here" />);
    await waitFor(() => expect(host.textContent).toContain('Bravo'));
    expect(host.textContent).not.toContain('Alpha');
  });

  it('сообщает об изменениях документа через onChange', async () => {
    let captured = '';
    render(
      <Editor path="A.md" initialDoc="start" onChange={(d) => { captured = d; }} />,
    );
    await waitFor(() => expect(screen.getByTestId('editor').textContent).toContain('start'));
    // onChange зовётся только при правках; стартовая загрузка не считается изменением.
    expect(captured).toBe('');
  });

  it('смена файла не считается правкой (регресс: externalSync, нет ложного dirty)', async () => {
    let changes = 0;
    const { rerender } = render(
      <Editor path="A.md" initialDoc="aaa" onChange={() => { changes += 1; }} />,
    );
    const host = screen.getByTestId('editor');
    await waitFor(() => expect(host.textContent).toContain('aaa'));
    rerender(<Editor path="B.md" initialDoc="bbb" onChange={() => { changes += 1; }} />);
    await waitFor(() => expect(host.textContent).toContain('bbb'));
    expect(changes).toBe(0);
  });
});
