import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ContradictionsPanel } from './ContradictionsPanel';
import { useAiFeaturesStore } from '../../stores/aiFeatures';
import { useContradictionsStore } from '../../stores/contradictions';
import { useUIStore } from '../../stores/ui';

// Реальный load() стора — disabled-кейс подменяет его no-op'ом, восстанавливаем после каждого теста.
const realLoad = useContradictionsStore.getState().load;

beforeEach(() => {
  // Фича включена — проверяем кнопку «Найти» и список (отдельный кейс ниже проверяет disabled-состояние).
  useAiFeaturesStore.setState({ contradictions: true });
});

afterEach(() => {
  vi.restoreAllMocks();
  useUIStore.setState({ contradictionsOpen: false });
  useAiFeaturesStore.setState({ contradictions: false });
  useContradictionsStore.setState({
    items: [],
    loading: false,
    generating: false,
    error: null,
    baseline: null,
    load: realLoad,
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

  // owner-тоггл OFF: кнопка «Найти» скрыта (была бы no-op), пустое состояние — честная подсказка.
  it('тоггл OFF → кнопка «Найти» скрыта + подсказка «включите в настройках»', async () => {
    useAiFeaturesStore.setState({ contradictions: false });
    // load() в no-op, чтобы остаться в пустом состоянии (мок иначе наполнил бы список 2 парами).
    useContradictionsStore.setState({ items: [], loading: false, load: async () => {} });
    useUIStore.setState({ contradictionsOpen: true });
    render(<ContradictionsPanel />);
    expect(screen.queryByTitle(/Найти|Find/i)).not.toBeInTheDocument();
    expect(await screen.findByText(/выключен|is off/i)).toBeInTheDocument();
  });

  // audit B10: модалка получила focus-trap → Esc закрывает её (а не «проваливается» в reading-mode).
  it('Esc закрывает модалку (focus-trap, audit B10)', async () => {
    useUIStore.setState({ contradictionsOpen: true });
    render(<ContradictionsPanel />);
    await screen.findByText(/одна заметка устарела/i);
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(useUIStore.getState().contradictionsOpen).toBe(false);
  });
});
