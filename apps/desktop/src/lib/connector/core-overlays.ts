/**
 * Легализация оверлеев ядра (F-8c) через реестр `overlays` — заменяет 7 хардкод-строк App.tsx
 * (`{goalsOpen && <GoalsPanel/>}` … `{contradictionsOpen && <ContradictionsPanel/>}`). Это НЕ модуль
 * (каркас) — ядровые оверлеи регистрируются напрямую, как `registerCoreViews` для main-вью. Питает
 * `OverlayOutlet` (рендер открытых оверлеев, каждый через ErrorBoundary).
 *
 * Behavior-preserving: `component`/порядок перенесены КАК ЕСТЬ из App.tsx (order 10..70 сохраняет
 * прежний DOM-порядок goals→memory→episodes→tasks→inbox→digest→contradictions — важно для стекинга
 * независимых floats digest/contradictions поверх trap-оверлеев). `isOpen` — существующие `*Open`-були
 * ui-стора (F-8c читает их КАК ЕСТЬ; перенос стейта в модули — вырезание F-10b).
 *
 * Регистрация — сайд-эффект при импорте (idempotent Map-реестр). App.tsx импортирует модуль ради
 * него, поэтому реестр заполнен до первого рендера OverlayOutlet (в т.ч. в юнит-тестах <App/>).
 */
import { ContradictionsPanel } from '../../components/contradictions/ContradictionsPanel';
import { DigestPanel } from '../../components/digest/DigestPanel';
import { EpisodesPanel } from '../../components/episodes/EpisodesPanel';
import { GoalsPanel } from '../../components/goals/GoalsPanel';
import { InboxPanel } from '../../components/inbox/InboxPanel';
import { MemoryPanel } from '../../components/memory/MemoryPanel';
import { TasksPanel } from '../../components/tasks/TasksPanel';
import { overlayRegistry } from './registries';

let registered = false;

/** Регистрирует 7 ядровых оверлеев (goals/memory/episodes/tasks/inbox/digest/contradictions) в реестр
 *  `overlays`. Идемпотентно. F-10b вырежет каждый в свой модуль (`ctx.overlays.register`). */
export function registerCoreOverlays(): void {
  if (registered) return;
  registered = true;

  overlayRegistry.register({
    id: 'goals',
    titleKey: 'commands.view.goals',
    order: 10,
    isOpen: (s) => s.goalsOpen,
    component: GoalsPanel,
  });
  overlayRegistry.register({
    id: 'memory',
    titleKey: 'commands.view.memory',
    order: 20,
    isOpen: (s) => s.memoryOpen,
    component: MemoryPanel,
  });
  overlayRegistry.register({
    id: 'episodes',
    titleKey: 'episode.title',
    order: 30,
    isOpen: (s) => s.episodesOpen,
    component: EpisodesPanel,
  });
  overlayRegistry.register({
    id: 'tasks',
    titleKey: 'commands.view.tasks',
    order: 40,
    isOpen: (s) => s.tasksOpen,
    component: TasksPanel,
  });
  overlayRegistry.register({
    id: 'inbox',
    titleKey: 'commands.view.inbox',
    order: 50,
    isOpen: (s) => s.inboxOpen,
    component: InboxPanel,
  });
  overlayRegistry.register({
    id: 'digest',
    titleKey: 'commands.view.digest',
    order: 60,
    isOpen: (s) => s.digestOpen,
    component: DigestPanel,
  });
  overlayRegistry.register({
    id: 'contradictions',
    titleKey: 'commands.view.contradictions',
    order: 70,
    isOpen: (s) => s.contradictionsOpen,
    component: ContradictionsPanel,
  });
}

registerCoreOverlays();
