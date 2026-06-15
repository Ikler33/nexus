import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { tauriApi, type MemoryFact } from '../lib/tauri-api';
import { MEM_CAP, staleFactIds, useMemoryStore } from './memory';

function fact(p: Partial<MemoryFact> & { id: number }): MemoryFact {
  return {
    id: p.id,
    text: p.text ?? `f${p.id}`,
    pinned: p.pinned ?? false,
    source: p.source ?? 'explicit',
    createdAt: p.createdAt ?? 0,
    usedAt: p.usedAt ?? 0,
  };
}

beforeEach(() => {
  useMemoryStore.setState({ facts: [], loading: false });
});
afterEach(() => vi.restoreAllMocks());

describe('memory store (MEM-4)', () => {
  it('load заполняет facts; ошибка → пустой список без throw', async () => {
    vi.spyOn(tauriApi.memory, 'list').mockResolvedValue([fact({ id: 1 })]);
    await useMemoryStore.getState().load();
    expect(useMemoryStore.getState().facts).toHaveLength(1);
    expect(useMemoryStore.getState().loading).toBe(false);

    vi.spyOn(tauriApi.memory, 'list').mockRejectedValue(new Error('db down'));
    await useMemoryStore.getState().load();
    expect(useMemoryStore.getState().facts).toEqual([]);
  });

  it('add(explicit) триммит, зовёт memory.add и перечитывает', async () => {
    const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue(1);
    const list = vi.spyOn(tauriApi.memory, 'list').mockResolvedValue([fact({ id: 1, text: 'факт' })]);
    await useMemoryStore.getState().add('  факт  ');
    expect(add).toHaveBeenCalledWith('факт', 'explicit');
    expect(list).toHaveBeenCalled();
  });

  it('add пустого текста — no-op (команда не зовётся)', async () => {
    const add = vi.spyOn(tauriApi.memory, 'add').mockResolvedValue(1);
    await useMemoryStore.getState().add('   ');
    expect(add).not.toHaveBeenCalled();
  });

  it('setPinned/edit/remove дёргают команды и перечитывают', async () => {
    const sp = vi.spyOn(tauriApi.memory, 'setPinned').mockResolvedValue();
    const ed = vi.spyOn(tauriApi.memory, 'edit').mockResolvedValue();
    const rm = vi.spyOn(tauriApi.memory, 'delete').mockResolvedValue();
    const list = vi.spyOn(tauriApi.memory, 'list').mockResolvedValue([]);
    await useMemoryStore.getState().setPinned(3, true);
    await useMemoryStore.getState().edit(3, '  новый  ');
    await useMemoryStore.getState().remove(3);
    expect(sp).toHaveBeenCalledWith(3, true);
    expect(ed).toHaveBeenCalledWith(3, 'новый');
    expect(rm).toHaveBeenCalledWith(3);
    expect(list).toHaveBeenCalledTimes(3); // reload после каждой мутации
  });
});

describe('staleFactIds (D6: подсветка сверх капа)', () => {
  it('нет переполнения → пустой Set', () => {
    expect(staleFactIds([fact({ id: 1 }), fact({ id: 2, pinned: true })]).size).toBe(0);
  });

  it('переполнение → подсвечены наименее свежие не-пины сверх капа; пины не считаются', () => {
    const facts: MemoryFact[] = [fact({ id: 9999, pinned: true, usedAt: 0 })];
    // MEM_CAP+2 не-пинов с возрастающим usedAt → 2 самых старых (id 0,1) сверх капа.
    for (let i = 0; i < MEM_CAP + 2; i++) facts.push(fact({ id: i, usedAt: i + 1 }));
    const stale = staleFactIds(facts);
    expect(stale.size).toBe(2);
    expect(stale.has(0)).toBe(true);
    expect(stale.has(1)).toBe(true);
    expect(stale.has(9999)).toBe(false); // пин не подсвечивается и не считается к капу
  });
});
