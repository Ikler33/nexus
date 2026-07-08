/**
 * Легализация main-вью ядра (F-8) через реестр `views` поверх mainView-enum F-4. Это НЕ модуль
 * (каркас) — ядровые вью регистрируются напрямую, как `registerCoreCommands` для команд. Питает
 * `MainViewOutlet` (App-lookup) и кнопки ActivityBar (home/today). Редактор — дефолт-вью
 * (не в ActivityBar: вход через дерево/сайдбар). news (F-9), board (F-10c) и agent (F-11) — уже НЕ
 * здесь: вырезаны в модули `connector/modules/{news,board,agent}` и регистрируются через `ctx.views`
 * (эталон вью-модуля).
 *
 * Порядок/иконки/titleKey перенесены КАК ЕСТЬ из прежнего тернарника App.tsx и хардкода ActivityBar
 * (behavior-preserving). Нав-действия — существующие экшены ui-стора (openHome/… — не setMainView,
 * чтобы сохранить точную семантику).
 *
 * Регистрация — сайд-эффект при импорте (idempotent Map-реестр). App.tsx импортирует модуль ради
 * него, поэтому реестр заполнен до первого рендера ActivityBar/MainViewOutlet (в т.ч. в юнит-тестах,
 * рендерящих <App/> напрямую, минуя main.tsx).
 */
import { CalendarCheck, FileText, Home } from 'lucide-react';
import { HomeView } from '../../components/home/HomeView';
import { TodayView } from '../../components/today/TodayView';
import { EditorArea } from '../../components/workspace/EditorArea';
import { useUIStore } from '../../stores/ui';
import { viewRegistry } from './registries';

let registered = false;

/** Регистрирует ядровые main-вью (home/today) + редактор в реестр `views`. news (F-9), board (F-10c)
 *  и agent (F-11) — отдельные модули, не здесь. Идемпотентно. */
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
  // board (order 40) — БОЛЬШЕ НЕ ядровая вью: вырезана в модуль `connector/modules/board` (F-10c).
  // Регистрируется через `ctx.views`; в ActivityBar встаёт между «Новости»=30 и «Агент»=50.
  // agent (order 50) — БОЛЬШЕ НЕ ядровая вью: вырезана в модуль `connector/modules/agent` (F-11,
  // самая связанная фича). Регистрируется через `ctx.views` (lazy AgentView + suspense) при активации
  // модуля; в ActivityBar встаёт между «Доска»=40 и «Редактор»=100. Команда `view.agent` — там же.
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
