import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { commands } from '../../lib/commands';
import { useUIStore } from '../../stores/ui';
import { CommandPalette } from './CommandPalette';

beforeEach(() => {
  commands._reset();
  useUIStore.setState({ paletteOpen: false });
});
afterEach(() => commands._reset());

describe('CommandPalette (Ф0-8)', () => {
  it('закрыта по умолчанию', () => {
    render(<CommandPalette />);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('открывается, фильтрует и выполняет команду по Enter', () => {
    let ran = '';
    commands.register({ id: 'a', title: 'Alpha command', run: () => { ran = 'a'; } });
    commands.register({ id: 'b', title: 'Beta command', run: () => { ran = 'b'; } });
    useUIStore.getState().openPalette();
    render(<CommandPalette />);

    expect(screen.getByRole('dialog')).toBeInTheDocument();
    const input = screen.getByRole('combobox');
    fireEvent.change(input, { target: { value: 'Beta' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    expect(ran).toBe('b');
    expect(useUIStore.getState().paletteOpen).toBe(false);
  });

  it('Esc закрывает палитру', () => {
    commands.register({ id: 'a', title: 'Alpha', run: () => {} });
    useUIStore.getState().openPalette();
    render(<CommandPalette />);
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });
    expect(useUIStore.getState().paletteOpen).toBe(false);
  });

  // DP-5 (макет palette.jsx): непустой запрос ищет и файлы — секция «Файлы», Enter открывает.
  it('секция «Файлы»: запрос находит заметку, Enter открывает её', async () => {
    const { useVaultStore } = await import('../../stores/vault');
    const { activePath, useWorkspaceStore } = await import('../../stores/workspace');
    await useVaultStore.getState().openVault('');
    useUIStore.setState({ paletteOpen: true });
    render(<CommandPalette />);

    const input = screen.getByRole('combobox');
    fireEvent.change(input, { target: { value: 'Roadmap' } });
    expect(await screen.findByText(/^файлы$|^files$/i)).toBeInTheDocument();
    const fileRow = await screen.findByText('Projects/Roadmap.md');
    expect(fileRow).toBeInTheDocument();

    // Первый ряд (файл) активен → Enter открывает заметку.
    fireEvent.keyDown(input, { key: 'Enter' });
    await vi.waitFor(() =>
      expect(activePath(useWorkspaceStore.getState())).toBe('Projects/Roadmap.md'),
    );
  });
});
