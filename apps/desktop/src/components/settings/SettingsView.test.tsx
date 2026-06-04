import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { SettingsView } from './SettingsView';
import { useUIStore } from '../../stores/ui';

describe('SettingsView (кросс-план #11, оболочка раздела)', () => {
  it('рендерит нав секций и переключает их', () => {
    useUIStore.setState({ settingsSection: 'appearance' });
    render(<SettingsView />);

    // Левый нав — 4 секции.
    expect(screen.getByRole('button', { name: /оформление|appearance/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /модели|models/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /горячие|hotkeys/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /о программе|about/i })).toBeInTheDocument();

    // Активна «Оформление» → видны контролы темы.
    expect(screen.getByText(/тема|theme/i)).toBeInTheDocument();

    // Переключаемся на «О программе» → секция меняется в ui-сторе и видна версия/vault.
    fireEvent.click(screen.getByRole('button', { name: /о программе|about/i }));
    expect(useUIStore.getState().settingsSection).toBe('about');
    expect(screen.getByText(/версия|version/i)).toBeInTheDocument();
  });
});
