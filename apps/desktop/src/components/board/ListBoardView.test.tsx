import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ListBoardView } from './ListBoardView';
import i18n from '../../i18n/setup';
import type { TaskCard } from '../../lib/tauri-api';

const cards: TaskCard[] = [
  {
    path: 'Tasks/A.md',
    title: 'Alpha',
    status: 'todo',
    project: 'Nexus',
    priority: 'high',
    due: '2026-06-20',
    tags: ['design'],
  },
  {
    path: 'Tasks/B.md',
    title: 'Beta',
    status: 'doing',
    project: 'Home',
    priority: 'low',
    due: null,
    tags: ['быт'],
  },
  {
    path: 'Tasks/C.md',
    title: 'Gamma',
    status: 'custom', // вне дефолтного набора — проверяем columnLabel
    project: 'Nexus',
    priority: null,
    due: '2026-06-14', // просрочено относительно today
    tags: [],
  },
];

/** Локализация статуса как у доски: кастомный 'custom' → дружелюбная метка. */
const columnLabel = (id: string) => (id === 'custom' ? 'Ожидание' : id);

/** Порядок видимых строк по заголовку (строки — кнопки, чьё имя содержит заголовок задачи). */
function rowOrder(): string[] {
  return screen
    .getAllByRole('button')
    .map((b) => b.textContent || '')
    .filter((t) => /Alpha|Beta|Gamma/.test(t))
    .map((t) => t.match(/Alpha|Beta|Gamma/)![0]);
}

function renderList(onOpen = vi.fn()) {
  render(
    <ListBoardView cards={cards} today="2026-06-18" onOpen={onOpen} columnLabel={columnLabel} />,
  );
  return onOpen;
}

describe('ListBoardView (VIEW-1)', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en');
  });

  it('рендерит строку на каждую задачу + columnLabel для статуса вне набора', () => {
    renderList();
    expect(screen.getByText('Alpha')).toBeInTheDocument();
    expect(screen.getByText('Beta')).toBeInTheDocument();
    expect(screen.getByText('Gamma')).toBeInTheDocument();
    // Кастомный статус 'custom' отрисован дружелюбной меткой В СТРОКЕ.
    const gammaRow = screen.getByRole('button', { name: /Gamma/ });
    expect(gammaRow.textContent).toContain('Ожидание');
  });

  it('дефолт — сорт по due asc: просроченный первый, null-due последний', () => {
    renderList();
    expect(rowOrder()).toEqual(['Gamma', 'Alpha', 'Beta']); // 06-14, 06-20, null
  });

  it('клик по заголовку due переключает направление (desc), null-due ВСЁ РАВНО последний', () => {
    renderList();
    fireEvent.click(screen.getByRole('button', { name: 'Due' }));
    expect(rowOrder()).toEqual(['Alpha', 'Gamma', 'Beta']); // 06-20, 06-14, null
  });

  it('фильтр по проекту сужает строки', () => {
    renderList();
    fireEvent.change(screen.getByRole('combobox', { name: 'Project' }), {
      target: { value: 'Home' },
    });
    expect(rowOrder()).toEqual(['Beta']);
  });

  it('текстовый фильтр сужает по заголовку/тегам (CI)', () => {
    renderList();
    fireEvent.change(screen.getByRole('textbox', { name: 'Search tasks…' }), {
      target: { value: 'design' },
    });
    expect(rowOrder()).toEqual(['Alpha']); // тег design
  });

  it('пусто-после-фильтра показывает сообщение', () => {
    renderList();
    fireEvent.change(screen.getByRole('textbox', { name: 'Search tasks…' }), {
      target: { value: 'нет-такого' },
    });
    expect(screen.getByText('No tasks match the filter')).toBeInTheDocument();
    expect(rowOrder()).toEqual([]);
  });

  it('клик по строке вызывает onOpen с путём задачи', () => {
    const onOpen = renderList();
    fireEvent.click(screen.getByRole('button', { name: /Alpha/ }));
    expect(onOpen).toHaveBeenCalledWith('Tasks/A.md');
  });
});
