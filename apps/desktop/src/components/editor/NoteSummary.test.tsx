import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { NoteSummary } from './NoteSummary';

afterEach(() => vi.restoreAllMocks());

describe('NoteSummary (Inspector «Резюме»)', () => {
  it('запрашивает и показывает резюме текущего текста', async () => {
    const spy = vi.spyOn(tauriApi.suggest, 'noteSummary').mockResolvedValue('Краткое резюме заметки.');
    render(<NoteSummary doc="полный текст заметки" path="A.md" />);
    expect(await screen.findByText('Краткое резюме заметки.')).toBeInTheDocument();
    expect(spy).toHaveBeenCalledWith('полный текст заметки');
  });

  it('пустой ответ → честная заглушка', async () => {
    vi.spyOn(tauriApi.suggest, 'noteSummary').mockResolvedValue(null);
    render(<NoteSummary doc="текст" path="A.md" />);
    expect(await screen.findByText(/Нет резюме/)).toBeInTheDocument();
  });

  it('кнопка «Обновить» перегенерирует по актуальному тексту', async () => {
    const spy = vi.spyOn(tauriApi.suggest, 'noteSummary').mockResolvedValue('Резюме v1.');
    render(<NoteSummary doc="текст" path="A.md" />);
    await screen.findByText('Резюме v1.');
    expect(spy).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole('button', { name: 'Обновить' }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
  });

  it('смена заметки (path) перезапрашивает резюме С ТЕКСТОМ НОВОЙ заметки', async () => {
    const spy = vi
      .spyOn(tauriApi.suggest, 'noteSummary')
      .mockImplementation((t: string) => Promise.resolve(`резюме: ${t}`));
    const { rerender } = render(<NoteSummary doc="текст A" path="A.md" />);
    expect(await screen.findByText('резюме: текст A')).toBeInTheDocument();
    rerender(<NoteSummary doc="текст B" path="B.md" />);
    expect(await screen.findByText('резюме: текст B')).toBeInTheDocument();
    expect(spy).toHaveBeenNthCalledWith(1, 'текст A');
    expect(spy).toHaveBeenNthCalledWith(2, 'текст B');
  });
});
