import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { MemoryPanel } from './MemoryPanel';
import type { MemoryFact } from '../../lib/tauri-api';
import { useMemoryStore } from '../../stores/memory';
import { useUIStore } from '../../stores/ui';

function fact(p: Partial<MemoryFact> & { id: number }): MemoryFact {
  return {
    id: p.id,
    text: p.text ?? `f${p.id}`,
    pinned: p.pinned ?? false,
    source: p.source ?? 'explicit',
    createdAt: 0,
    usedAt: 0,
  };
}

/** Подменяем экшены стора моками — компонент читает их из состояния. */
function stubActions() {
  useMemoryStore.setState({
    load: vi.fn().mockResolvedValue(undefined),
    add: vi.fn().mockResolvedValue(undefined),
    setPinned: vi.fn().mockResolvedValue(undefined),
    edit: vi.fn().mockResolvedValue(undefined),
    remove: vi.fn().mockResolvedValue(undefined),
  });
}

beforeEach(() => {
  useMemoryStore.setState({ facts: [], loading: false });
  stubActions();
});
afterEach(() => vi.restoreAllMocks());

describe('MemoryPanel (MEM-4, AC-MEM-7)', () => {
  it('пустая память → подсказка пустого состояния', () => {
    render(<MemoryPanel />);
    expect(screen.getByText(/память пуста/i)).toBeInTheDocument();
  });

  it('рендерит факты; клик по пину дёргает setPinned', () => {
    const setPinned = vi.fn().mockResolvedValue(undefined);
    useMemoryStore.setState({
      facts: [fact({ id: 1, text: 'пишу на Rust' }), fact({ id: 2, text: 'дедлайн', pinned: true })],
      setPinned,
    });
    render(<MemoryPanel />);
    expect(screen.getByText('пишу на Rust')).toBeInTheDocument();
    expect(screen.getByText('дедлайн')).toBeInTheDocument();
    fireEvent.click(screen.getAllByLabelText(/закрепить/i)[0]); // не-пин → «Закрепить»
    expect(setPinned).toHaveBeenCalledWith(1, true);
  });

  it('ручное добавление: ввод + «Добавить» зовёт add', () => {
    const add = vi.fn().mockResolvedValue(undefined);
    useMemoryStore.setState({ add });
    render(<MemoryPanel />);
    fireEvent.change(screen.getByPlaceholderText(/новый факт/i), {
      target: { value: 'я из Тбилиси' },
    });
    fireEvent.click(screen.getByRole('button', { name: /добавить/i }));
    expect(add).toHaveBeenCalledWith('я из Тбилиси');
  });

  it('правка на месте: «Изменить» → input → «Сохранить» зовёт edit', () => {
    const edit = vi.fn().mockResolvedValue(undefined);
    useMemoryStore.setState({ facts: [fact({ id: 5, text: 'старый' })], edit });
    render(<MemoryPanel />);
    fireEvent.click(screen.getByLabelText(/изменить/i));
    fireEvent.change(screen.getByLabelText(/текст факта/i), {
      target: { value: 'новый текст' },
    });
    fireEvent.click(screen.getByLabelText(/^сохранить$/i));
    expect(edit).toHaveBeenCalledWith(5, 'новый текст');
  });

  it('Escape во время правки отменяет правку, НЕ закрывая панель (focus-trap не срабатывает)', () => {
    const closeSpy = vi.fn();
    useUIStore.setState({ closeMemory: closeSpy });
    useMemoryStore.setState({ facts: [fact({ id: 5, text: 'старый' })] });
    render(<MemoryPanel />);
    fireEvent.click(screen.getByLabelText(/изменить/i));
    fireEvent.keyDown(screen.getByLabelText(/текст факта/i), { key: 'Escape' });
    // Правка отменена (input исчез, снова текст), панель НЕ закрыта (trap onClose не зван).
    expect(screen.queryByLabelText(/текст факта/i)).toBeNull();
    expect(screen.getByText('старый')).toBeInTheDocument();
    expect(closeSpy).not.toHaveBeenCalled();
  });

  it('удаление с подтверждением зовёт remove', () => {
    const remove = vi.fn().mockResolvedValue(undefined);
    useMemoryStore.setState({ facts: [fact({ id: 7, text: 'удалить меня' })], remove });
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(<MemoryPanel />);
    fireEvent.click(screen.getByLabelText(/удалить/i));
    expect(remove).toHaveBeenCalledWith(7);
  });
});
