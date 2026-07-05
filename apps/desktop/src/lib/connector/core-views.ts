/**
 * Легализация main-вью ядра (F-8) через реестр `views` поверх mainView-enum F-4. Это НЕ модуль
 * (каркас) — ядровые вью регистрируются напрямую, как `registerCoreCommands` для команд. Питает
 * `MainViewOutlet` (App-lookup) и кнопки ActivityBar (home/today/board/agent). Редактор — дефолт-вью
 * (не в ActivityBar: вход через дерево/сайдбар). news (F-9) — уже НЕ здесь: вырезана в модуль
 * `connector/modules/news` и регистрируется через `ctx.views` (эталон вырезания модуля).
 *
 * Порядок/иконки/titleKey перенесены КАК ЕСТЬ из прежнего тернарника App.tsx и хардкода ActivityBar
 * (behavior-preserving). Нав-действия — существующие экшены ui-стора (openHome/… — не setMainView,
 * чтобы сохранить точную семантику, вкл. P0-3-фикс `() => openAgent()` без MouseEvent-seed).
 *
 * Регистрация — сайд-эффект при импорте (idempotent Map-реестр). App.tsx импортирует модуль ради
 * него, поэтому реестр заполнен до первого рендера ActivityBar/MainViewOutlet (в т.ч. в юнит-тестах,
 * рендерящих <App/> напрямую, минуя main.tsx).
 */
import { lazy } from 'react';
import { CalendarCheck, FileText, Home, LayoutGrid } from 'lucide-react';
import { CometIcon } from '../../components/common/BrandGlyphs';
import { BoardView } from '../../components/board/BoardView';
import { HomeView } from '../../components/home/HomeView';
import { TodayView } from '../../components/today/TodayView';
import { EditorArea } from '../../components/workspace/EditorArea';
import { useUIStore } from '../../stores/ui';
import { viewRegistry } from './registries';

// Вкладка Агента (UI-1) грузится лениво — как в прежнем App.tsx (`lazy(() => import(...).AgentView)`).
const AgentView = lazy(() =>
  import('../../components/agent/AgentView').then((m) => ({ default: m.AgentView })),
);

let registered = false;

/** Регистрирует ядровые main-вью (home/today/board/agent) + редактор в реестр `views`. news —
 *  отдельный модуль (F-9), не здесь. Идемпотентно. */
export function registerCoreViews(): void {
  if (registered) return;
  registered = true;

  viewRegistry.register({
    id: 'home',
    titleKey: 'commands.view.home',
    icon: Home,
    order: 10,
    component: HomeView,
    activityBar: true,
    activate: () => useUIStore.getState().openHome(),
    isActive: (v) => v === 'home',
  });
  viewRegistry.register({
    id: 'today',
    titleKey: 'commands.view.today',
    icon: CalendarCheck,
    order: 20,
    component: TodayView,
    activityBar: true,
    activate: () => useUIStore.getState().openToday(),
    isActive: (v) => v === 'today',
  });
  // news (order 30) — БОЛЬШЕ НЕ ядровая вью: вырезана в модуль `connector/modules/news` (F-9, первый
  // пилот вырезания). Регистрируется через `ctx.views` при активации модуля; в ActivityBar встаёт на
  // прежнее место (сортировка по order → между «Сегодня»=20 и «Доска»=40).
  viewRegistry.register({
    id: 'board',
    titleKey: 'commands.view.board',
    icon: LayoutGrid,
    order: 40,
    component: BoardView,
    activityBar: true,
    activate: () => useUIStore.getState().openBoard(),
    isActive: (v) => v === 'board',
  });
  viewRegistry.register({
    id: 'agent',
    titleKey: 'commands.view.agent',
    icon: CometIcon,
    order: 50,
    component: AgentView,
    // AgentView — lazy(): явная Suspense-граница как в прежнем App.tsx (не полагаемся на неявную
    // root-suspension React 19). Оживляет ветку MainViewOutlet `view.suspense ?…` (adversarial F-8).
    suspense: true,
    activityBar: true,
    // P0-3-смоук: НЕ голая ссылка — onClick подставил бы MouseEvent в optional `seed` и `seed.trim()`
    // бросил бы TypeError (кнопка Castor «мертвела»). Обёртка гасит аргумент.
    activate: () => useUIStore.getState().openAgent(),
    isActive: (v) => v === 'agent',
  });
  viewRegistry.register({
    id: 'editor',
    titleKey: 'commands.view.editor',
    icon: FileText,
    order: 100,
    component: EditorArea,
    // Редактор — дефолт-вью, входа-кнопки в ActivityBar нет (файлы/сайдбар — свой тоггл).
    activityBar: false,
    activate: () => useUIStore.getState().setMainView('editor'),
    isActive: (v) => v === 'editor',
  });
}

registerCoreViews();
