import { describe, expect, it } from 'vitest';
import './modules'; // сайд-эффект: активирует 7 оверлей-модулей F-10b (единственный источник оверлеев)
import { overlayRegistry } from './registries';
import { useUIStore } from '../../stores/ui';
import type { UIState } from '../../stores/ui';
import type { OverlayContribution } from './types';

/**
 * Реестр оверлеев (F-8c): register/get/list-детерминизм/dispose/идемпотентность + легализация 7
 * оверлеев (goals/…/contradictions). После F-10b ВСЕ 7 — модули (`ctx.overlays`), core-overlays удалён;
 * набор собирается сайд-эффектом modules. Тест проверяет РЕЗУЛЬТАТ (все 7 present, порядок 10..70,
 * isOpen). Свои вклады — id с префиксом `t:` (каждый тест снимает свои dispose'ом; модульные не
 * трогаем). Изоляцию падающего оверлея через OverlayOutlet покрывает overlay-isolation.test.tsx.
 */

const nullComp = () => null;

function overlay(
  id: string,
  order: number,
  isOpen: (s: UIState) => boolean = () => false,
): OverlayContribution {
  return { id, titleKey: `t.${id}`, order, isOpen, component: nullComp };
}

describe('overlayRegistry (F-8c)', () => {
  it('register → get возвращает вклад; dispose удаляет', () => {
    const d = overlayRegistry.register(overlay('t:one', 5));
    expect(overlayRegistry.get('t:one')?.titleKey).toBe('t.t:one');
    d.dispose();
    expect(overlayRegistry.get('t:one')).toBeUndefined();
  });

  it('list() детерминирован — сортировка по order (независимо от порядка регистрации)', () => {
    const d3 = overlayRegistry.register(overlay('t:c', 300));
    const d1 = overlayRegistry.register(overlay('t:a', 100));
    const d2 = overlayRegistry.register(overlay('t:b', 200));
    const ids = overlayRegistry
      .list()
      .filter((o) => o.id.startsWith('t:'))
      .map((o) => o.id);
    expect(ids).toEqual(['t:a', 't:b', 't:c']);
    d1.dispose();
    d2.dispose();
    d3.dispose();
  });

  it('идемпотентность: повторная регистрация того же id заменяет, не дублирует', () => {
    const d1 = overlayRegistry.register(overlay('t:dup', 10));
    const d2 = overlayRegistry.register({ ...overlay('t:dup', 10), titleKey: 't.replaced' });
    const dups = overlayRegistry.list().filter((o) => o.id === 't:dup');
    expect(dups).toHaveLength(1);
    expect(dups[0].titleKey).toBe('t.replaced');
    d1.dispose();
    d2.dispose();
    expect(overlayRegistry.get('t:dup')).toBeUndefined();
  });

  it('оверлеи легализованы (7 модулей F-10b): 7 панелей present, порядок сохранён', () => {
    const coreIds = overlayRegistry
      .list()
      .filter((o) => !o.id.startsWith('t:'))
      .map((o) => o.id);
    // Порядок 10..70 — прежний DOM-порядок App.tsx (goals→…→contradictions).
    expect(coreIds).toEqual([
      'goals',
      'memory',
      'episodes',
      'tasks',
      'inbox',
      'digest',
      'contradictions',
    ]);
  });

  it('isOpen-селекторы читают соответствующие `*Open`-були ui-стора', () => {
    // Все закрыты → каждый isOpen=false.
    useUIStore.setState({
      goalsOpen: false,
      memoryOpen: false,
      episodesOpen: false,
      tasksOpen: false,
      inboxOpen: false,
      digestOpen: false,
      contradictionsOpen: false,
    });
    const closed = useUIStore.getState();
    for (const id of ['goals', 'memory', 'episodes', 'tasks', 'inbox', 'digest', 'contradictions']) {
      expect(overlayRegistry.get(id)?.isOpen(closed), `${id} closed`).toBe(false);
    }

    // Открываем ровно «Цели» → isOpen('goals')=true, остальные false (селектор читает свой флаг).
    useUIStore.setState({ goalsOpen: true });
    const goalsOn = useUIStore.getState();
    expect(overlayRegistry.get('goals')?.isOpen(goalsOn)).toBe(true);
    expect(overlayRegistry.get('digest')?.isOpen(goalsOn)).toBe(false);

    // И «Противоречия» независимо (float, может стоять вместе с trap-оверлеем).
    useUIStore.setState({ contradictionsOpen: true });
    const both = useUIStore.getState();
    expect(overlayRegistry.get('goals')?.isOpen(both)).toBe(true);
    expect(overlayRegistry.get('contradictions')?.isOpen(both)).toBe(true);

    useUIStore.setState({ goalsOpen: false, contradictionsOpen: false });
  });
});
