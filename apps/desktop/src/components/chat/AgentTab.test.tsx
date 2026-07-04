import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { AgentTab } from './AgentTab';
import { useUIStore } from '../../stores/ui';

/**
 * P1-11: Castor «Быстрый старт» — три пункта раньше все звали голый `openAgent()` (поле агента пустое →
 * пункты неотличимы). Теперь каждый сидит СВОЙ промпт в композер агента (prefill, не авто-отправка).
 * Проверяем: «Открыть раздел Агента» открывает агента без сида; каждый quick-start — с РАЗНЫМ промптом.
 */
function reset() {
  useUIStore.setState({ mainView: 'home', pendingAgentSeed: null });
}

beforeEach(reset);
afterEach(reset);

describe('AgentTab (Castor лаунчер, P1-11)', () => {
  it('«Открыть раздел Агента» открывает агента БЕЗ сида (просто вход)', () => {
    render(<AgentTab />);
    fireEvent.click(screen.getByRole('button', { name: /Открыть раздел Агента/ }));
    expect(useUIStore.getState().mainView).toBe('agent');
    expect(useUIStore.getState().pendingAgentSeed).toBeNull();
  });

  it('каждый пункт «Быстрого старта» сидит СВОЙ непустой промпт (3 пункта различимы)', () => {
    render(<AgentTab />);
    // Кнопки быстрого старта = всё, кроме «Открыть раздел Агента».
    const all = screen.getAllByRole('button');
    const quick = all.filter((b) => !/Открыть раздел Агента/.test(b.textContent ?? ''));
    expect(quick).toHaveLength(3);

    const seeds: string[] = [];
    for (const btn of quick) {
      useUIStore.setState({ mainView: 'home', pendingAgentSeed: null });
      fireEvent.click(btn);
      const seed = useUIStore.getState().pendingAgentSeed;
      expect(useUIStore.getState().mainView).toBe('agent'); // агент открылся
      expect(seed?.text.trim()).toBeTruthy(); // непустой промпт засеян
      seeds.push(seed!.text);
    }
    // Три РАЗНЫХ промпта (а не три одинаковых openAgent() — суть фикса P1-11).
    expect(new Set(seeds).size).toBe(3);
  });
});
