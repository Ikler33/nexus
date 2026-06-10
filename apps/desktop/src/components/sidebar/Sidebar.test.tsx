import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { useVaultStore } from '../../stores/vault';
import { useWorkspaceStore } from '../../stores/workspace';
import { Sidebar } from './Sidebar';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  useWorkspaceStore.getState().reset();
});

describe('Sidebar (Ф0-7)', () => {
  it('пустой запрос показывает дерево; ввод показывает результаты; очистка возвращает дерево', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);

    expect(await screen.findByText('Projects')).toBeInTheDocument(); // дерево

    const input = screen.getByLabelText('Поиск по vault');
    fireEvent.change(input, { target: { value: 'Roadmap' } });
    expect(await screen.findByText('Projects/Roadmap.md')).toBeInTheDocument(); // результат

    fireEvent.click(screen.getByLabelText('Очистить поиск'));
    expect(await screen.findByText('Projects')).toBeInTheDocument(); // снова дерево
  });

  it('поиск по тегу находит заметки', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.change(screen.getByLabelText('Поиск по vault'), { target: { value: 'planning' } });
    // #planning есть в Projects/Roadmap.md (mock CONTENT)
    expect(await screen.findByText('Projects/Roadmap.md')).toBeInTheDocument();
  });

  it('нет совпадений → пустое состояние', async () => {
    await useVaultStore.getState().openVault('');
    render(<Sidebar />);
    fireEvent.change(screen.getByLabelText('Поиск по vault'), { target: { value: 'zzzнет' } });
    expect(await screen.findByText('Ничего не найдено')).toBeInTheDocument();
  });
});
