import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { ConflictResolver } from './ConflictResolver';

describe('ConflictResolver (Ф4-8 / DP-10, макет conflict.jsx)', () => {
  // Мок mergePreview отдаёт 1 конфликт: до выбора — «не выбрано», apply заблокирован;
  // клик по стороне выбирает её (бейдж/прогресс), apply открывается и завершает merge.
  it('выбор стороны: прогресс, бейджи и гейт кнопки «Применить»', async () => {
    render(<ConflictResolver onClose={() => {}} />);

    expect(await screen.findByText(/разрешено 0 из 1|0 of 1 resolved/i)).toBeInTheDocument();
    expect(screen.getByText(/не выбрано|unresolved/i)).toBeInTheDocument();
    const apply = screen.getByRole('button', { name: /применить и запушить|apply and push/i });
    expect(apply).toBeDisabled();

    // Клик по стороне «С диска» (их версия) — выбор зафиксирован.
    fireEvent.click(screen.getByRole('button', { name: /их правка той же строки/i }));
    expect(await screen.findByText(/разрешено 1 из 1|1 of 1 resolved/i)).toBeInTheDocument();
    expect(apply).toBeEnabled();

    fireEvent.click(apply);
    expect(await screen.findByText(/слито и запушено|merged and pushed/i)).toBeInTheDocument();
  });

  // Bulk-кнопка «Везде локальные» разрешает все конфликты одним кликом.
  it('bulk «Везде локальные» разрешает всё разом', async () => {
    render(<ConflictResolver onClose={() => {}} />);
    await screen.findByText(/разрешено 0 из 1|0 of 1 resolved/i);
    fireEvent.click(screen.getByRole('button', { name: /везде локальные|all local/i }));
    expect(await screen.findByText(/разрешено 1 из 1|1 of 1 resolved/i)).toBeInTheDocument();
    vi.restoreAllMocks();
  });

  // audit B10: резолвер получил focus-trap → Esc вызывает onClose (а не «проваливается» в reading-mode).
  it('Esc вызывает onClose (focus-trap, audit B10)', async () => {
    const onClose = vi.fn();
    render(<ConflictResolver onClose={onClose} />);
    await screen.findByText(/разрешено 0 из 1|0 of 1 resolved/i);
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(onClose).toHaveBeenCalled();
    vi.restoreAllMocks();
  });
});
