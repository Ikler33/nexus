import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { Sidebar } from './Sidebar';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  useWorkspaceStore.getState().reset();
});

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

  // DP-2: панель «Теги» из list_tags; клик по тегу = поиск по нему.
  it('панель «Теги»: список с количеством; клик по тегу запускает поиск', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.click(screen.getByRole('tab', { name: /теги|tags/i }));
    const tag = await screen.findByRole('button', { name: /planning/ });
    fireEvent.click(tag);
    // Переключились в поиск с query=planning → результат по тегу.
    expect(await screen.findByText('Projects/Roadmap.md')).toBeInTheDocument();
  });

  // DP-2: «Избранное» — пустое состояние с подсказкой.
  it('панель «Избранное»: пустое состояние', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.click(screen.getByRole('tab', { name: /избранное|starred/i }));
    expect(await screen.findByText(/избранного пока нет|nothing starred/i)).toBeInTheDocument();
  });
});
