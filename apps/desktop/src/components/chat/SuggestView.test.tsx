import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SuggestView } from './SuggestView';
import { tauriApi } from '../../lib/tauri-api';
import { useSuggestStore } from '../../stores/suggest';
import { useWorkspaceStore } from '../../stores/workspace';

beforeEach(() => {
  useSuggestStore.setState({ path: null, items: [], loading: false });
  vi.restoreAllMocks();
});

describe('SuggestView (Ф1-9)', () => {
  it('без активного файла — подсказка', () => {
    useWorkspaceStore.setState({
      buffers: {},
      groups: [{ id: 'g0', tabs: [], activeTab: null }],
      activeGroupId: 'g0',
    });
    render(<SuggestView />);
    expect(screen.getByText(/Откройте заметку/)).toBeInTheDocument();
  });

  it('показывает карточки и скрывает по «Скрыть»', async () => {
    vi.spyOn(tauriApi.suggest, 'forFile').mockResolvedValue([
      { path: 'sv-B.md', title: null, score: 0.8, reason: 'почему B' },
    ]);
    useWorkspaceStore.setState({
      buffers: { 'sv.md': { path: 'sv.md', doc: '# A', dirty: false } },
      groups: [{ id: 'g0', tabs: ['sv.md'], activeTab: 'sv.md' }],
      activeGroupId: 'g0',
    });

    render(<SuggestView />);
    expect(await screen.findByText('sv-B.md')).toBeInTheDocument();
    expect(screen.getByText('80%')).toBeInTheDocument();
    expect(screen.getByText('почему B')).toBeInTheDocument();

    fireEvent.click(screen.getByText('Скрыть'));
    expect(screen.queryByText('sv-B.md')).not.toBeInTheDocument();
  });
});
