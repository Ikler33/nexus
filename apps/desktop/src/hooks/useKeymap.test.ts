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

  // FILE-RENAME-COMMAND: F-клавиши диспетчеризуются голыми (F2 = переименование, OS-стандарт).
  it('голая функц. клавиша F2 с зарегистрированной командой → запускает её', () => {
    vi.spyOn(commands, 'resolve').mockReturnValue('file.rename');
    const run = vi.spyOn(commands, 'run').mockResolvedValue(undefined);
    renderHook(() => useKeymap());
    const e = new KeyboardEvent('keydown', { key: 'F2', cancelable: true });
    window.dispatchEvent(e);
    expect(run).toHaveBeenCalledWith('file.rename');
    expect(e.defaultPrevented).toBe(true);
  });

  it('голая F-клавиша без команды (F5-reload) → НЕ перехватываем (preventDefault не зовём)', () => {
    vi.spyOn(commands, 'resolve').mockReturnValue(undefined);
    const run = vi.spyOn(commands, 'run').mockResolvedValue(undefined);
    renderHook(() => useKeymap());
    const e = new KeyboardEvent('keydown', { key: 'F5', cancelable: true });
    window.dispatchEvent(e);
    expect(run).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  // Ревью-находка: window-листенер ловит F2 даже из инпута (нативное всплытие сквозь stopPropagation) —
  // в формовом поле F2 НЕ должна перезапускать команду (иначе rename-input пере-сидит введённое имя).
  it('голая F2 из формового поля (input) → игнор (защита rename-input/поиска)', () => {
    vi.spyOn(commands, 'resolve').mockReturnValue('file.rename');
    const run = vi.spyOn(commands, 'run').mockResolvedValue(undefined);
    renderHook(() => useKeymap());
    const input = document.createElement('input');
    document.body.appendChild(input);
    const e = new KeyboardEvent('keydown', { key: 'F2', cancelable: true, bubbles: true });
    input.dispatchEvent(e); // всплывёт до window, но e.target === input
    expect(run).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
    document.body.removeChild(input);
  });
});
