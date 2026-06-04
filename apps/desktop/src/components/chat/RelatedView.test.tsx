import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { RelatedView } from './RelatedView';
import { tauriApi } from '../../lib/tauri-api';
import { useRelatedStore } from '../../stores/related';
import { useWorkspaceStore } from '../../stores/workspace';

afterEach(() => {
  vi.restoreAllMocks();
  useRelatedStore.setState({ path: null, items: [], loading: false, threshold: 0 });
});

describe('RelatedView (#35)', () => {
  it('рендерит похожие для активной заметки; «вставить» не убирает строку (AC-RN-6)', async () => {
    useWorkspaceStore.setState({
      buffers: { 'A.md': { path: 'A.md', doc: '', dirty: false } },
      groups: [{ id: 'g0', tabs: ['A.md'], activeTab: 'A.md' }],
      activeGroupId: 'g0',
    });
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([
      { path: 'B.md', title: 'Заметка B', score: 0.9, reason: 'причина' },
    ]);
    render(<RelatedView />);

    expect(await screen.findByText('Заметка B')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /вставить|insert/i }));
    expect(screen.getByText('Заметка B')).toBeInTheDocument(); // строка осталась
    expect(useWorkspaceStore.getState().buffers['A.md'].doc).toContain('[[B]]');
  });
});
