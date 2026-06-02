import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
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
    const input = screen.getByLabelText('Команда');
    fireEvent.change(input, { target: { value: 'Beta' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    expect(ran).toBe('b');
    expect(useUIStore.getState().paletteOpen).toBe(false);
  });

  it('Esc закрывает палитру', () => {
    commands.register({ id: 'a', title: 'Alpha', run: () => {} });
    useUIStore.getState().openPalette();
    render(<CommandPalette />);
    fireEvent.keyDown(screen.getByLabelText('Команда'), { key: 'Escape' });
    expect(useUIStore.getState().paletteOpen).toBe(false);
  });
});
