import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { tauriApi } from '../../lib/tauri-api';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { Sidebar } from './Sidebar';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  useWorkspaceStore.getState().reset();
});
afterEach(() => vi.restoreAllMocks());

describe('Sidebar (Ф0-7 / DP-2)', () => {
  it('панель «Файлы» по умолчанию — дерево; rail «Поиск» → ввод → результаты', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);

    expect(await screen.findByText('Projects')).toBeInTheDocument(); // дерево

    fireEvent.click(screen.getByRole('tab', { name: /поиск|search/i }));
    const input = screen.getByLabelText('Поиск по vault');
    fireEvent.change(input, { target: { value: 'Roadmap' } });
    expect(await screen.findByText('Projects/Roadmap.md')).toBeInTheDocument(); // результат

    // Назад в «Файлы» — снова дерево.
    fireEvent.click(screen.getByRole('tab', { name: /файлы|files/i }));
    expect(await screen.findByText('Projects')).toBeInTheDocument();
  });

  it('поиск по тегу находит заметки', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.click(screen.getByRole('tab', { name: /поиск|search/i }));
    fireEvent.change(screen.getByLabelText('Поиск по vault'), { target: { value: 'planning' } });
    // #planning есть в Projects/Roadmap.md (mock CONTENT)
    expect(await screen.findByText('Projects/Roadmap.md')).toBeInTheDocument();
  });

  it('нет совпадений → пустое состояние', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.click(screen.getByRole('tab', { name: /поиск|search/i }));
    fireEvent.change(screen.getByLabelText('Поиск по vault'), { target: { value: 'zzzнет' } });
    expect(await screen.findByText('Ничего не найдено')).toBeInTheDocument();
  });

  // Клик по тегу = ТОЧНЫЙ фильтр (notesByTag), а не зашумлённый substring-поиск (searchVault).
  it('клик по тегу → exact-фильтр (notesByTag, не searchVault) + чип тега', async () => {
    const notesByTag = vi.spyOn(tauriApi.vault, 'notesByTag');
    const searchVault = vi.spyOn(tauriApi.search, 'searchVault');
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.click(screen.getByRole('tab', { name: /теги|tags/i }));
    fireEvent.click(await screen.findByRole('button', { name: /planning/ }));

    expect(await screen.findByText('Projects/Roadmap.md')).toBeInTheDocument();
    expect(notesByTag).toHaveBeenCalledWith('planning');
    expect(searchVault).not.toHaveBeenCalled(); // точный фильтр, НЕ substring-поиск
    // Чип активного тега со снятием (×).
    expect(screen.getByRole('button', { name: /снять фильтр|clear tag/i })).toBeInTheDocument();
  });

  it('снятие чипа тега (×) → выход из тег-режима (подсказка поиска)', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.click(screen.getByRole('tab', { name: /теги|tags/i }));
    fireEvent.click(await screen.findByRole('button', { name: /planning/ }));
    await screen.findByText('Projects/Roadmap.md');
    fireEvent.click(screen.getByRole('button', { name: /снять фильтр|clear tag/i }));
    // Тег снят → пустой поиск → подсказка, чипа нет.
    expect(await screen.findByText(/поиск по заголовкам|searches titles/i)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /снять фильтр|clear tag/i })).not.toBeInTheDocument();
  });

  it('ввод текста при активном теге → выход в обычный поиск ДРУГОЙ заметки (без stale-лика)', async () => {
    const searchVault = vi.spyOn(tauriApi.search, 'searchVault');
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.click(screen.getByRole('tab', { name: /теги|tags/i }));
    fireEvent.click(await screen.findByRole('button', { name: /planning/ }));
    await screen.findByText('Projects/Roadmap.md'); // тег-результат (#planning)
    // Печатаем запрос ДРУГОЙ заметки (Idea НЕ в #planning) — дискриминирует текст-путь от stale тега.
    fireEvent.change(screen.getByLabelText('Поиск по vault'), { target: { value: 'Idea' } });
    expect(await screen.findByText('Notes/Idea.md')).toBeInTheDocument(); // результат именно текст-поиска
    expect(screen.queryByText('Projects/Roadmap.md')).not.toBeInTheDocument(); // тег-результат снят, не залип
    expect(searchVault).toHaveBeenCalledWith('Idea');
    expect(screen.queryByRole('button', { name: /снять фильтр|clear tag/i })).not.toBeInTheDocument();
  });

  // DP-2: «Избранное» — пустое состояние с подсказкой.
  it('панель «Избранное»: пустое состояние', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.click(screen.getByRole('tab', { name: /избранное|starred/i }));
    expect(await screen.findByText(/избранного пока нет|nothing starred/i)).toBeInTheDocument();
  });
});
