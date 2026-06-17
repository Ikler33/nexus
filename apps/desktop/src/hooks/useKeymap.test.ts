import { renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { commands } from '../lib/commands';
import { useKeymap } from './useKeymap';

afterEach(() => vi.restoreAllMocks());

describe('useKeymap', () => {
  it('модификатор-комбо с зарегистрированной командой → запускает её', () => {
    vi.spyOn(commands, 'resolve').mockReturnValue('some.cmd');
    const run = vi.spyOn(commands, 'run').mockResolvedValue(undefined);
    renderHook(() => useKeymap());
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'g', metaKey: true, cancelable: true }));
    expect(run).toHaveBeenCalledWith('some.cmd');
  });

  it('defaultPrevented → команду НЕ дублируем (ближний хендлер уже обработал, напр. ⌘G поиска в CM6)', () => {
    vi.spyOn(commands, 'resolve').mockReturnValue('some.cmd');
    const run = vi.spyOn(commands, 'run').mockResolvedValue(undefined);
    renderHook(() => useKeymap());
    const e = new KeyboardEvent('keydown', { key: 'g', metaKey: true, cancelable: true });
    e.preventDefault(); // имитируем обработку фокусным компонентом
    window.dispatchEvent(e);
    expect(run).not.toHaveBeenCalled();
  });

  it('без модификатора → игнор (обычный ввод текста)', () => {
    const run = vi.spyOn(commands, 'run').mockResolvedValue(undefined);
    renderHook(() => useKeymap());
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'g', cancelable: true }));
    expect(run).not.toHaveBeenCalled();
  });
});
