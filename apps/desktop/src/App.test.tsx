import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from './App';
import { useUIStore } from './stores/ui';
import { useVaultStore } from './stores/vault';
import { useWorkspaceStore } from './stores/workspace';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  useWorkspaceStore.getState().reset();
  useUIStore.setState({ homeOpen: true, newsOpen: false });
});

describe('App (Ф0-3 / Ф4-11 / DP-1)', () => {
  it('первый запуск: онбординг → «Открыть vault» показывает файловое дерево', async () => {
    render(<App />);
    // Без vault — приветственный экран онбординга (Ф4-11).
    fireEvent.click(await screen.findByRole('button', { name: /Открыть vault/ }));
    expect(await screen.findByText('README.md')).toBeInTheDocument();
    expect(screen.getByText('Projects')).toBeInTheDocument();
  });

  it('после открытия vault стартовая вью — HOME-дашборд (DP-1); файл из дерева → редактор', async () => {
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: /Открыть vault/ }));
    await screen.findByText('README.md');
    // Лендинг — Home: приветствие + hero-поиск.
    expect(
      screen.getByText(/добрый день|доброе утро|добрый вечер|доброй ночи|good/i),
    ).toBeInTheDocument();
    // Открытие файла из дерева закрывает Home и показывает редактор.
    fireEvent.click(screen.getByText('README.md'));
    expect(await screen.findByRole('tab', { name: /README/ })).toBeInTheDocument();
    expect(useUIStore.getState().homeOpen).toBe(false);
  });
});
