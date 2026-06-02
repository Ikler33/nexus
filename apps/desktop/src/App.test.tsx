import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from './App';
import { useVaultStore } from './stores/vault';
import { useWorkspaceStore } from './stores/workspace';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {}, notes: [] });
  useWorkspaceStore.getState().reset();
});

describe('App (Ф0-3)', () => {
  it('автооткрывает мок-vault и показывает файловое дерево', async () => {
    render(<App />);
    expect(await screen.findByText('README.md')).toBeInTheDocument();
    expect(screen.getByText('Projects')).toBeInTheDocument();
  });

  it('до выбора файла показывает подсказку', async () => {
    render(<App />);
    await screen.findByText('README.md');
    expect(screen.getByText(/Выберите файл/)).toBeInTheDocument();
  });
});
