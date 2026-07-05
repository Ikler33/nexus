/**
 * Легализация оверлеев ядра (F-8c) через реестр `overlays` — заменяет 7 хардкод-строк App.tsx
 * (`{goalsOpen && <GoalsPanel/>}` … `{contradictionsOpen && <ContradictionsPanel/>}`). Это НЕ модуль
 * (каркас) — ядровые оверлеи регистрируются напрямую, как `registerCoreViews` для main-вью. Питает
 * `OverlayOutlet` (рендер открытых оверлеев, каждый через ErrorBoundary).
 *
 * Behavior-preserving: `component`/порядок перенесены КАК ЕСТЬ из App.tsx (order 10..70 сохраняет
 * прежний DOM-порядок goals→memory→episodes→tasks→inbox→digest→contradictions — важно для стекинга
 * независимых floats digest/contradictions поверх trap-оверлеев). `isOpen` — существующие `*Open`-були
 * ui-стора (читаются КАК ЕСТЬ).
 *
 * F-10b ВЫРЕЗАЕТ эти оверлеи по одному в свои модули (`connector/modules/<feature>`) через
 * `ctx.overlays.register` — по мере выреза запись уходит отсюда (см. комментарии-заглушки). ПАТТЕРН
 * оверлей-модуля: стейт видимости `*Open` + действия open/close/toggle ОСТАЮТСЯ ядром (ui-стор, как
 * `mainView`), модуль лишь регистрирует КОМПОНЕНТ + `isOpen`-селектор поверх ядрового флага. Остаток
 * (ещё не вырезанное) регистрируется тут напрямую, как `registerCoreViews` для main-вью.
 *
 * Регистрация — сайд-эффект при импорте (idempotent Map-реестр). App.tsx импортирует модуль ради
 * него, поэтому реестр заполнен до первого рендера OverlayOutlet (в т.ч. в юнит-тестах <App/>).
 */
import { ContradictionsPanel } from '../../components/contradictions/ContradictionsPanel';
import { DigestPanel } from '../../components/digest/DigestPanel';
import { InboxPanel } from '../../components/inbox/InboxPanel';
import { TasksPanel } from '../../components/tasks/TasksPanel';
import { overlayRegistry } from './registries';

let registered = false;

/** Регистрирует ещё НЕ вырезанные ядровые оверлеи в реестр `overlays`. Идемпотентно. F-10b вырезает
 *  каждый в свой модуль (`ctx.overlays.register`) — вырезанные помечены комментарием-заглушкой. */
export function registerCoreOverlays(): void {
  if (registered) return;
  registered = true;

  // goals, memory, episodes — вырезаны в модули `connector/modules/*` (F-10b): через `ctx.overlays`.
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
