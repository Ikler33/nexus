import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { useVaultStore } from '../../stores/vault';
import { FileTree } from './FileTree';

beforeEach(() => {
  useVaultStore.setState({
    info: null,
    childrenByPath: {},
    expanded: {},
    loading: {},
    selectedPath: null,
  });
});

describe('FileTree (Ф0-3)', () => {
  it('рендерит корневые узлы открытого vault', async () => {
    await useVaultStore.getState().openVault('');
    render(<FileTree />);
    expect(await screen.findByText('README.md')).toBeInTheDocument();
    expect(screen.getByText('Projects')).toBeInTheDocument();
    expect(screen.getByRole('tree')).toBeInTheDocument();
  });

  it('клик по папке лениво раскрывает её', async () => {
    await useVaultStore.getState().openVault('');
    render(<FileTree />);
    fireEvent.click(await screen.findByText('Projects'));
    expect(await screen.findByText('Roadmap.md')).toBeInTheDocument();
  });

  it('клик по файлу открывает его в редакторе', async () => {
    await useVaultStore.getState().openVault('');
    render(<FileTree />);
    fireEvent.click(await screen.findByText('README.md'));
    await waitFor(() => {
      expect(useVaultStore.getState().activeFile?.path).toBe('README.md');
      expect(useVaultStore.getState().selectedPath).toBe('README.md');
    });
  });
});
