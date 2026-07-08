import { useTranslation } from 'react-i18next';
import { panelRegistry } from '../../lib/connector';
import type { PanelPlacement } from '../../lib/connector';
import { ErrorBoundary } from '../common/ErrorBoundary';

/**
 * Рендер workspace-панели тела через реестр `panels` коннектора (F-12, легализация хардкода App.tsx
 * `import { AiPanel }` + 3-вариантного рендера). Для каждой зарегистрированной панели рендерит компонент
 * в позиции `variant` (side/bottom/overlay — ведётся pref `aiLayout`, ядро-chrome) через
 * per-contribution ErrorBoundary (падение панели → плашка, app жив — цель F-8). `key` по id.
 *
 * ВИДИМОСТЬ и ПОЗИЦИЯ — ЯДРО (App): App вычисляет составное условие показа
 * (`chatOpen && !reading && mainView==='editor'`) + активный `variant` из pref и ставит нужную обёртку
 * (`.aiBottom`/`.aiScrim`) + рефлоу грида `.appBody` — outlet рендерит только компонент(ы) реестра.
 * Так App больше НЕ импортирует `components/chat` (граница F-1b), а модуль даёт лишь компонент.
 *
 * В проде реестр содержит РОВНО один вклад (chat/AiPanel); `list()` (не `get(id)`) — чтобы outlet не
 * знал конкретного id панели (реестр отдаёт вклады как ссылки). Слот single-purpose: видимость общая
 * (chat-стейт ядра) — вклады рендерятся списком внутри уже открытого слота, своей видимости у них нет.
 */
export function AiPanelOutlet({ variant }: { variant: PanelPlacement }) {
  const { t } = useTranslation();
  return (
    <>
      {panelRegistry.list().map((panel) => {
        const Panel = panel.component;
        return (
          <ErrorBoundary key={panel.id} label={t(panel.titleKey)}>
            <Panel variant={variant} />
          </ErrorBoundary>
        );
      })}
    </>
  );
}
