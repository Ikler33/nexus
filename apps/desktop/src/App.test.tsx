import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from './App';
import { useVaultStore } from './stores/vault';
import { useWorkspaceStore } from './stores/workspace';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {}, notes: [] });
  useWorkspaceStore.getState().reset();
});

describe('App (Ф0-3 / Ф4-11)', () => {
  it('первый запуск: онбординг → «Открыть vault» показывает файловое дерево', async () => {
    render(<App />);
    // Без vault — приветственный экран онбординга (Ф4-11).
    fireEvent.click(await screen.findByRole('button', { name: /Открыть vault/ }));
    expect(await screen.findByText('README.md')).toBeInTheDocument();
    expect(screen.getByText('Projects')).toBeInTheDocument();
  });

  it('после открытия vault до выбора файла показывает подсказку', async () => {
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: /Открыть vault/ }));
    await screen.findByText('README.md');
    expect(screen.getByText(/Выберите файл/)).toBeInTheDocument();
  });
});
