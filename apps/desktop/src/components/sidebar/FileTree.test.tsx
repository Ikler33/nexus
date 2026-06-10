import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { useVaultStore } from '../../stores/vault';
import { activePath, useWorkspaceStore } from '../../stores/workspace';
import { FileTree } from './FileTree';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  useWorkspaceStore.getState().reset();
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
});
