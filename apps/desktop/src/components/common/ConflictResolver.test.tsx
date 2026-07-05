import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { ConflictResolver } from './ConflictResolver';

describe('ConflictResolver (Ф4-8 / DP-10 / QASR-views, макет conflict.jsx)', () => {
  // Мок mergePreview отдаёт 1 конфликт: до выбора — «не выбрано», apply заблокирован;
  // клик по стороне выбирает её (бейдж/прогресс в футере), apply открывается и завершает merge.
  it('выбор стороны: прогресс, бейджи и гейт кнопки «Применить»', async () => {
    render(<ConflictResolver onClose={() => {}} />);

    // Прогресс «N из M» теперь в футере (grid-area foot).
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

  // Bulk-кнопка «Везде локальные» (теперь в правом рейле) разрешает все конфликты одним кликом.
  it('bulk «Везде локальные» разрешает всё разом', async () => {
    render(<ConflictResolver onClose={() => {}} />);
    await screen.findByText(/разрешено 0 из 1|0 of 1 resolved/i);
    fireEvent.click(screen.getByRole('button', { name: /везде локальные|all local/i }));
    expect(await screen.findByText(/разрешено 1 из 1|1 of 1 resolved/i)).toBeInTheDocument();
    vi.restoreAllMocks();
  });

  // QASR-views: правый рейл = stats-боксы + навигатор; клик по записи навигатора не падает.
  it('рейл: stats-боксы и навигатор по конфликтам', async () => {
    render(<ConflictResolver onClose={() => {}} />);
    await screen.findByText(/разрешено 0 из 1|0 of 1 resolved/i);

    // Навигатор (aria-label) содержит запись «Конфликт 1» + путь файла.
    const nav = screen.getByRole('complementary', { name: /навигатор конфликтов|conflict navigator/i });
    const jump = within(nav).getByRole('button', { name: /конфликт 1|conflict 1/i });
    expect(jump).toBeInTheDocument();
    // Stats-боксы рендерят метки «правки здесь / правки на диске».
    expect(within(nav).getByText(/правки здесь|local edits/i)).toBeInTheDocument();
    expect(within(nav).getByText(/правки на диске|disk edits/i)).toBeInTheDocument();

    // Клик по навигатору (scroll + flash) не должен бросать.
    fireEvent.click(jump);
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
