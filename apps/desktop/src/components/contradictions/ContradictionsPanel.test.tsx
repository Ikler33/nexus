import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ContradictionsPanel } from './ContradictionsPanel';
import { useContradictionsStore } from '../../stores/contradictions';
import { useUIStore } from '../../stores/ui';

afterEach(() => {
  vi.restoreAllMocks();
  useUIStore.setState({ contradictionsOpen: false });
  useContradictionsStore.setState({
    items: [],
    loading: false,
    generating: false,
    error: null,
    baseline: null,
  });
});

describe('ContradictionsPanel (#vision)', () => {
  it('рендерит найденные противоречия (пара + тип + объяснение)', async () => {
    useUIStore.setState({ contradictionsOpen: true });
    render(<ContradictionsPanel />);

    // Мок отдаёт 2 противоречия (temporal + hard).
    expect(await screen.findByText(/одна заметка устарела/i)).toBeInTheDocument();
    expect(screen.getByText(/прямое противоречие|вынос/i)).toBeInTheDocument();
    expect(screen.getByText(/устарело|outdated/i)).toBeInTheDocument(); // бейдж типа temporal
  });

  it('кнопка «Найти» ставит поиск и показывает прогресс', async () => {
    useUIStore.setState({ contradictionsOpen: true });
    render(<ContradictionsPanel />);
    await screen.findByText(/одна заметка устарела/i);

    fireEvent.click(screen.getByTitle(/Найти|Find/i));
    expect(useContradictionsStore.getState().generating).toBe(true);
    expect(await screen.findByText(/Ищу…|Searching…/i)).toBeInTheDocument();
  });
});
