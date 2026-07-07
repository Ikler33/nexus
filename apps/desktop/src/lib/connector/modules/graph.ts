/**
 * F-10d — «Граф ссылок» (ADR-004) как вырезанный оверлей-модуль через overlays-реестр (F-8c) с
 * mount:'appBody'. Отличие от 8 прежних оверлеев (F-10b/F-10c, все mount:'app'): граф — полноэкранный
 * слой ВНУТРИ тела `.appBody` (`position:absolute; inset:0`), НЕ поверх титлбара/статусбара (фикс
 * владельца «хром торчал поверх графа»). Точку монтирования несёт поле `mount` `OverlayContribution`
 * (F-10d, МИНИМАЛЬНО app|appBody) — `OverlayOutlet(mount:'appBody')` внутри `.appBody` рендерит его
 * туда, где раньше был хардкод `{graphOpen && <div.graphLayer><GraphView/></div>}` в App.tsx.
 *
 * ПАТТЕРН оверлей-модуля (v0, как goals/sync): стейт видимости `graphOpen` + `openGraph/closeGraph/
 * toggleGraph` + Esc-прецедент (`selectReadingEscBlocked`) ОСТАЮТСЯ ядром (ui-стор) — модуль даёт
 * КОМПОНЕНТ (`GraphLayer` = ленивый GraphView под Suspense + слой-обёртка) + `isOpen`-селектор +
 * команду палитры. Кнопка «Граф» в ActivityBar остаётся ядро-chrome (зовёт `toggleGraph()` ui-стора).
 *
 * ГРАНИЦА (F-1b): App.tsx больше НЕ импортит `components/graph` (убран `lazy(()=>import(GraphView))`);
 * панель приходит из реестра `overlays`. `GraphLayer` живёт в graph-зоне и лениво тянет `GraphView`
 * (тяжёлый d3-force/louvain остаётся в отдельном чанке). grep-инвариант «ядро не импортит
 * components/graph» держится eslint-ом F-1b (`graph` в MODULE_FEATURES).
 */
import { GraphLayer } from '../../../components/graph/GraphLayer';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

/** Модуль «Граф ссылок». Оверлей (mount:'appBody') GraphLayer + команда палитры (F-10d). */
export const graphModule: NexusModule = {
  id: 'graph',
  activate(ctx) {
    // Оверлей: mount:'appBody' (единственный такой) — слой внутри тела, не покрывает хром. order=90
    // (после sync=80; в своей mount-группе он один, порядок косметичен). isOpen читает ядровой `graphOpen`.
    ctx.overlays.register({
      id: 'graph',
      titleKey: 'commands.view.graph',
      order: 90,
      isOpen: (s) => s.graphOpen,
      component: GraphLayer,
      mount: 'appBody',
    });

    // Команда палитры (прежняя commands-core `view.graph`): `ctx.commands` префиксует id →
    // `graph:view.graph`, source=plugin. Хоткей ⌘G сохранён КАК ЕСТЬ (resolve матчит defaultKey
    // независимо от source); пара `view.graph`→`graph:view.graph` в COMMAND_ID_ALIASES (lib/commands.ts).
    ctx.commands.register({
      id: 'view.graph',
      title: 'Local graph',
      titleKey: 'commands.view.graph',
      defaultKey: 'mod+g',
      run: () => useUIStore.getState().toggleGraph(),
    });
  },
};
