import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { useUIStore } from '../../stores/ui';
import { useVaultStore } from '../../stores/vault';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { FileTree } from './FileTree';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  useWorkspaceStore.getState().reset();
  useUIStore.setState({ revealTarget: null, renameTarget: null });
});

describe('FileTree (Ф0-3/Ф0-9)', () => {
  it('рендерит корневые узлы открытого vault', async () => {
    await useVaultStore.getState().openVault('');
    render(<FileTree />);
    expect(await screen.findByText('README')).toBeInTheDocument(); // DP-15: без .md
    expect(screen.getByText('Projects')).toBeInTheDocument();
    expect(screen.getByRole('tree')).toBeInTheDocument();
  });

  it('клик по папке лениво раскрывает её', async () => {
    await useVaultStore.getState().openVault('');
    render(<FileTree />);
    fireEvent.click(await screen.findByText('Projects'));
    expect(await screen.findByText('Roadmap')).toBeInTheDocument(); // DP-15: без .md
  });

  it('клик по файлу открывает его в активной группе (workspace)', async () => {
    await useVaultStore.getState().openVault('');
    render(<FileTree />);
    fireEvent.click(await screen.findByText('README'));
    await waitFor(() => expect(activePath(useWorkspaceStore.getState())).toBe('README.md'));
  });

  // audit B10: при схлопывании дерева active-индекс не должен «повисать» за пределами списка —
  // иначе aria-activedescendant ссылается на несуществующий treeitem. Кламп держит его валидным.
  it('REVEAL-ACTIVE-FILE: requestReveal делает строку файла активной (после раскрытия предков)', async () => {
    await useVaultStore.getState().openVault('');
    await useVaultStore.getState().revealPath('Projects/Roadmap.md'); // раскрыли предков
    render(<FileTree />);
    await screen.findByText('Roadmap');
    act(() => useUIStore.getState().requestReveal('Projects/Roadmap.md'));
    await waitFor(() => {
      const row = screen.getByText('Roadmap').closest('[role="treeitem"]');
      expect(row).toHaveAttribute('data-active');
    });
    expect(useUIStore.getState().revealTarget).toBeNull(); // запрос сброшен после скролла
  });

  // FILE-RENAME-COMMAND: requestRename открывает инлайн-input с именем файла без .md, сбрасывает запрос.
  it('FILE-RENAME-COMMAND: requestRename открывает инлайн-переименование строки файла', async () => {
    await useVaultStore.getState().openVault('');
    await useVaultStore.getState().revealPath('Projects/Roadmap.md');
    render(<FileTree />);
    await screen.findByText('Roadmap');
    act(() => useUIStore.getState().requestRename('Projects/Roadmap.md'));
    const input = await screen.findByDisplayValue('Roadmap'); // имя без .md в input
    expect(input.tagName).toBe('INPUT');
    expect(useUIStore.getState().renameTarget).toBeNull(); // запрос сброшен после открытия input
  });

  it('кламп active при схлопывании дерева (a11y, audit B10)', async () => {
    await useVaultStore.getState().openVault('');
    render(<FileTree />);
    const tree = screen.getByRole('tree');
    fireEvent.click(await screen.findByText('Projects')); // раскрыли → узлов стало больше
    await screen.findByText('Roadmap');
    // уводим active вниз — move() упрётся в последний узел РАЗВЁРНУТОГО дерева
    for (let i = 0; i < 8; i++) fireEvent.keyDown(tree, { key: 'ArrowDown' });
    fireEvent.click(screen.getByText('Projects')); // схлопнули → узлов снова мало
    await waitFor(() => {
      const id = tree.getAttribute('aria-activedescendant') as string;
      expect(document.getElementById(id)).not.toBeNull(); // указывает на реально существующий узел
    });
  });
});
