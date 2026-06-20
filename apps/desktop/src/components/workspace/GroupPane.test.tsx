import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GroupPane } from './GroupPane';
import { useWorkspaceStore } from '../../stores/workspace';

// Пустая группа (без вкладок): таб-стрип с back/forward рендерится всегда, без CM6-редактора —
// изолируем проверку nav-кнопок от ленивого превью/Editor.
function setupNav(navHistory: { path: string; groupId: string }[], navIndex: number) {
  useWorkspaceStore.setState({
    groups: [{ id: 'g0', tabs: [], activeTab: null }],
    activeGroupId: 'g0',
    buffers: {},
    navHistory,
    navIndex,
  });
}

beforeEach(() => useWorkspaceStore.getState().reset());
afterEach(() => vi.restoreAllMocks());

describe('GroupPane back/forward (NAV-3 кнопки)', () => {
  it('пустая история → обе кнопки disabled', () => {
    setupNav([], -1);
    render(<GroupPane groupId="g0" />);
    expect(screen.getByRole('button', { name: 'Назад' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Вперёд' })).toBeDisabled();
  });

  it('на левом крае истории: Назад disabled, Вперёд активна', () => {
    setupNav([{ path: 'A.md', groupId: 'g0' }, { path: 'B.md', groupId: 'g0' }], 0);
    render(<GroupPane groupId="g0" />);
    expect(screen.getByRole('button', { name: 'Назад' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Вперёд' })).toBeEnabled();
  });

  it('на правом крае истории: Назад активна, Вперёд disabled', () => {
    setupNav([{ path: 'A.md', groupId: 'g0' }, { path: 'B.md', groupId: 'g0' }], 1);
    render(<GroupPane groupId="g0" />);
    expect(screen.getByRole('button', { name: 'Назад' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Вперёд' })).toBeDisabled();
  });

  it('клик «Назад» зовёт существующий navBack стора (логика не дублируется)', () => {
    setupNav([{ path: 'A.md', groupId: 'g0' }, { path: 'B.md', groupId: 'g0' }], 1);
    const navBack = vi.spyOn(useWorkspaceStore.getState(), 'navBack').mockResolvedValue();
    render(<GroupPane groupId="g0" />);
    fireEvent.click(screen.getByRole('button', { name: 'Назад' }));
    expect(navBack).toHaveBeenCalledTimes(1);
  });

  it('клик «Вперёд» зовёт существующий navForward стора', () => {
    setupNav([{ path: 'A.md', groupId: 'g0' }, { path: 'B.md', groupId: 'g0' }], 0);
    const navForward = vi.spyOn(useWorkspaceStore.getState(), 'navForward').mockResolvedValue();
    render(<GroupPane groupId="g0" />);
    fireEvent.click(screen.getByRole('button', { name: 'Вперёд' }));
    expect(navForward).toHaveBeenCalledTimes(1);
  });
});
