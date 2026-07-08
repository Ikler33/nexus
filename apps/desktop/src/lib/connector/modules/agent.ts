/**
 * F-11 — «Агент» (Castor) как вырезанный модуль через views-реестр (F-9 news / F-10c board — эталон
 * вью-модуля). Agent — полноэкранная main-вью (mainView='agent'), НЕ оверлей: вклад идёт через
 * `ctx.views` (кнопка ActivityBar + MainViewOutlet) + команда палитры через `ctx.commands`. Ядро
 * (core-views/commands-core/App.tsx) больше НЕ импортирует `components/agent` — вклады отдаёт реестр
 * коннектора. Это САМАЯ связанная фича серии F; вырез строго behavior-preserving.
 *
 * ОТЛИЧИЕ от news-эталона: команда `view.agent` — ТОГГЛ (toggleAgent), а НЕ отдельная секция настроек
 * (у агента её нет — настройки агента живут в ОБЩЕЙ ai-секции SettingsView, ядро-chrome, НЕ вырезаются).
 * Как у news, id команды префиксуется модулем: `view.agent` → `agent:view.agent`, source=plugin;
 * пара `view.agent`→`agent:view.agent` добавлена в COMMAND_ID_ALIASES (`lib/commands.ts`) — ручной
 * хоткей пользователя на старый id ремапится (иначе no-op).
 *
 * ПАТТЕРН (как news/board/оверлеи F-10b): стейт видимости main-вью (`mainView`) + экшены
 * open/close/toggleAgent + **seed-handoff** (`pendingAgentSeed`/`consumeAgentSeed`, P1-11 «Быстрый
 * старт») ОСТАЮТСЯ ЯДРОМ (ui-стор) — модуль лишь даёт КОМПОНЕНТ + нав-действие + команду. Домен-стор
 * `stores/agent.ts` тоже остаётся в `stores/` как data-слой (импортируется ТОЛЬКО из components/agent,
 * как `stores/news.ts`; инвариант выреза — про `components/agent`, не про data-слой). См. docs/dev/connector.md.
 *
 * AgentView грузится ЛЕНИВО (как в прежнем core-views): явная Suspense-граница (`suspense:true`) —
 * не полагаемся на неявную root-suspension React 19. Behavior-preserving: order=50/icon=CometIcon/
 * titleKey/activate перенесены КАК ЕСТЬ из прежней записи core-views (между «Доска»=40 и «Редактор»=100).
 *
 * Rust/tauri-команды агента — ВНЕ скоупа F-11 (вырезается только фронт).
 */
import { lazy } from 'react';
import { CometIcon } from '../../../components/common/BrandGlyphs';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

// Вкладка Агента грузится лениво — как в прежнем core-views (`lazy(() => import(...).AgentView)`).
const AgentView = lazy(() =>
  import('../../../components/agent/AgentView').then((m) => ({ default: m.AgentView })),
);

/** Модуль «Агент» (Castor). Вырезан из core-views/commands-core через `ctx.views`+`ctx.commands` (F-11). */
export const agentModule: NexusModule = {
  id: 'agent',
  activate(ctx) {
    // Main-вью + кнопка ActivityBar. order=50 (между «Доска»=40 и «Редактор»=100) — как в core-views.
    ctx.views.register({
      id: 'agent',
      titleKey: 'commands.view.agent',
      icon: CometIcon,
      order: 50,
      component: AgentView,
      // AgentView — lazy(): явная Suspense-граница как в прежнем core-views (не полагаемся на неявную
      // root-suspension React 19). Оживляет ветку MainViewOutlet `view.suspense ?…` (adversarial F-8).
      suspense: true,
      activityBar: true,
      // P0-3-смоук: НЕ голая ссылка — onClick подставил бы MouseEvent в optional `seed` и `seed.trim()`
      // бросил бы TypeError (кнопка Castor «мертвела»). Обёртка гасит аргумент.
      activate: () => useUIStore.getState().openAgent(),
      isActive: (v) => v === 'agent',
    });

    // Команда палитры (прежняя commands-core `view.agent`). `ctx.commands` префиксует id модулем →
    // фактический id `agent:view.agent`, source=`plugin`. Палитра ищет по названию (titleKey) — путь
    // пользователя не меняется. `toggleAgent` — тоггл-семантика прежней команды.
    ctx.commands.register({
      id: 'view.agent',
      title: 'Agent',
      titleKey: 'commands.view.agent',
      run: () => useUIStore.getState().toggleAgent(),
    });
  },
};
