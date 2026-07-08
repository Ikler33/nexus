/**
 * F-12 — «AI-панель» (chat) как вырезанный модуль через НОВЫЙ реестр `panels` (F-12) + `ctx.commands`.
 * ФИНАЛ модуляризации фронта (13-й модуль): панель Чат/Castor (`components/chat/AiPanel`) вырезана из
 * ядра — App.tsx больше НЕ импортирует `components/chat`, компонент отдаёт реестр `panels`.
 *
 * ПОЧЕМУ НОВЫЙ РЕЕСТР (а не views/overlays): AI-панель — НЕ полноэкранная main-вью (`views`:
 * взаимоисключаемый `mainView`; панель СОСУЩЕСТВУЕТ с «Редактором») и НЕ обычный оверлей (`overlays`:
 * единый `isOpen(UIState)`-селектор, `<Component/>` без пропов, float поверх тела). Панель докается в
 * теле в ТРЁХ позициях (pref `aiLayout`: side/bottom/overlay — `PanelPlacement`), `side`/`bottom` рефлоуят
 * грид `.appBody` (не float), видимость — составное ядровое выражение (`chatOpen && !reading &&
 * mainView==='editor'`), позиция/размер — из pref (не UIState). Шоехорнить это в `OverlayContribution`
 * = раздуть чистую оверлей-абстракцию понятиями variant/pref/layout, НУЖНЫМИ только чату (анти-YAGNI).
 * Поэтому, по прецеденту F-8c (7 оверлеев не легли на 5 реестров v0 → добавлен РОВНО оверлей-реестр),
 * добавлен РОВНО минимальный `panels`-слот (одна docked-панель, без обобщений). См. docs/dev/connector.md.
 *
 * ПАТТЕРН (как news/board/оверлеи F-10b/agent F-11): ВИДИМОСТЬ и ПОЗИЦИЯ — ЯДРО-chrome (App вычисляет
 * `aiVisible`/`variant` из ui+prefs, ставит скрим/рефлоу грида), стейт `chatOpen` + `open/close/toggleChat`
 * + `aiTab`/`setAiTab` остаются в ui-сторе, titlebar-чекбокс «AI-панель» зовёт `toggleChat()` ui-стора.
 * Модуль даёт ТОЛЬКО компонент панели + команду палитры. Домен-стор `stores/chat.ts` остаётся в `stores/`
 * как data-слой (его легально импортируют ядро/иные фичи: App-hydrate, Home/Today/Episodes — это
 * СТОР, не `components/chat`; инвариант выреза — про компонент-зону).
 *
 * AiPanel импортируется ЕАГЕРНО (как раньше в App.tsx — не lazy): манифест грузится сайд-эффектом
 * `modules/index.ts` до первого рендера, поведение загрузки идентично прежнему прямому импорту App.
 *
 * ОТЛИЧИЕ от news-эталона: вместо секции настроек — вклад в `panels`; команда `view.chat` — не тоггл
 * вью, а `setAiTab('chat')` + `openChat()` (как прежняя commands-core, defaultKey ⌘J сохранён). Как
 * везде, id команды префиксуется модулем: `view.chat` → `chat:view.chat`, source=plugin; пара
 * `view.chat`→`chat:view.chat` в COMMAND_ID_ALIASES (`lib/commands.ts`) — ручной хоткей юзера на старый
 * id ремапится (иначе no-op). Rust/tauri-команды чата — ВНЕ скоупа (вырезается только фронт).
 */
import { AiPanel } from '../../../components/chat/AiPanel';
import { useUIStore } from '../../../stores/ui';
import type { NexusModule } from '../types';

/** Модуль «AI-панель» (chat). Вырезан из App.tsx/commands-core через `ctx.panels`+`ctx.commands` (F-12). */
export const chatModule: NexusModule = {
  id: 'chat',
  activate(ctx) {
    // Workspace-панель тела. Позицию (variant из pref `aiLayout`) и видимость выбирает ЯДРО (App) —
    // модуль даёт компонент + i18n-ключ имени (плашка ErrorBoundary). Рендер — AiPanelOutlet.
    ctx.panels.register({
      id: 'chat',
      titleKey: 'chrome.aiPanel',
      component: AiPanel,
    });

    // Команда палитры (прежняя commands-core `view.chat`). `ctx.commands` префиксует id →
    // `chat:view.chat`, source=plugin. Хоткей ⌘J сохранён КАК ЕСТЬ (resolve матчит defaultKey
    // независимо от source); пара `view.chat`→`chat:view.chat` в COMMAND_ID_ALIASES.
    ctx.commands.register({
      id: 'view.chat',
      title: 'AI chat',
      titleKey: 'commands.view.chat',
      defaultKey: 'mod+j',
      run: () => {
        useUIStore.getState().setAiTab('chat');
        useUIStore.getState().openChat();
      },
    });
  },
};
