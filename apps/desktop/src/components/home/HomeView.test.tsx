import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { HomeView } from './HomeView';
import { useHomeStore } from '../../stores/home';
import { useUIStore } from '../../stores/ui';

function resetStores() {
  useUIStore.setState({ homeOpen: true, newsOpen: false });
  useHomeStore.setState({
    data: null,
    activity: null,
    brief: null,
    questions: [],
    drift: null,
    stale: [],
    graph: null,
    loading: true,
    generating: {},
    error: null,
  });
}

describe('HomeView (DP-1, макет home.jsx)', () => {
  beforeEach(resetStores);

  // Дашборд: приветствие, сводка дня (AI-карта из кэша виджета), недавние, статистика,
  // stale radar и открытые вопросы — всё из мок-бэкенда H1/H6/H2.
  it('рендерит секции лендинга из данных бэкенда', async () => {
    render(<HomeView />);

    expect(
      await screen.findByText(/архитектурой агентов/),
    ).toBeInTheDocument(); // сводка дня (bold-фрагмент внутри strong)
    expect(screen.getByText(/добр|good/i)).toBeInTheDocument(); // и «Доброй ночи» (тест в 23–06 ч)
    // «RAG Pipeline» встречается в continue-карте и в недавних.
    expect(screen.getAllByText('RAG Pipeline').length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText(/сводка дня|daily brief/i).length).toBeGreaterThan(0);
    expect(screen.getByText(/недавние|recent/i)).toBeInTheDocument();
    expect(screen.getByText(/статистика|stats/i)).toBeInTheDocument();
    expect(screen.getByText('Roadmap Q1')).toBeInTheDocument(); // stale radar
    expect(screen.getByText(/чанк-перекрытие/)).toBeInTheDocument(); // открытый вопрос
    expect(screen.getByText(/смещение фокуса|focus drift/i)).toBeInTheDocument();
    // Heatmap-сетка построена (17 недель × 7).
    expect(document.querySelectorAll('[class*="heatCell"]').length).toBeGreaterThan(119);
  });

  // Клик по недавней заметке открывает её в редакторе и закрывает Home.
  it('недавняя заметка → открытие файла, Home закрывается', async () => {
    render(<HomeView />);
    const row = await screen.findByRole('button', { name: /Embeddings/ });
    fireEvent.click(row);
    await vi.waitFor(() => expect(useUIStore.getState().homeOpen).toBe(false));
  });

  // «Обновить» на AI-карте ставит фоновую генерацию: thinking-оверлей до прихода результата.
  it('refresh AI-виджета показывает thinking и возвращает контент (мок)', async () => {
    render(<HomeView />);
    await screen.findByText(/архитектурой агентов/);
    const refreshButtons = screen.getAllByRole('button', { name: /обновить|refresh/i });
    fireEvent.click(refreshButtons[0]); // сводка дня
    expect(await screen.findByText(/анализирую|analyzing/i)).toBeInTheDocument();
    await vi.waitFor(
      () => expect(screen.queryByText(/анализирую|analyzing/i)).not.toBeInTheDocument(),
      { timeout: 3000 },
    );
  });
});
