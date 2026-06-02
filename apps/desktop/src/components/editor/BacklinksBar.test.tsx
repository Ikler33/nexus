import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { useVaultStore } from '../../stores/vault';
import { BacklinksBar } from './BacklinksBar';

beforeEach(() => {
  useVaultStore.setState({
    info: null,
    childrenByPath: {},
    expanded: {},
    loading: {},
    selectedPath: null,
    activeFile: null,
    dirty: false,
    notes: [],
  });
});

describe('BacklinksBar (Ф0-6)', () => {
  it('показывает входящие ссылки активного файла', async () => {
    await useVaultStore.getState().openVault('');
    await useVaultStore.getState().openFile('Inbox.md'); // на Inbox ссылается README
    render(<BacklinksBar />);
    expect(await screen.findByText('README.md')).toBeInTheDocument();
  });

  it('показывает пустое состояние, когда обратных ссылок нет', async () => {
    await useVaultStore.getState().openVault('');
    await useVaultStore.getState().openFile('Notes/Idea.md'); // на Idea никто не ссылается
    render(<BacklinksBar />);
    expect(await screen.findByText(/Нет обратных ссылок/)).toBeInTheDocument();
  });
});
