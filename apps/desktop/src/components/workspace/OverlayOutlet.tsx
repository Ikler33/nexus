import { useTranslation } from 'react-i18next';
import { overlayRegistry } from '../../lib/connector';
import type { OverlayMount } from '../../lib/connector';
import { useUIStore } from '../../stores/ui';
import { ErrorBoundary } from '../common/ErrorBoundary';

/**
 * Рендер открытых оверлеев через реестр `overlays` коннектора (F-8c, легализация 7 хардкод-строк
 * App.tsx `{xOpen && <Panel/>}`): для каждого зарегистрированного оверлея вычисляет `isOpen(state)`
 * и, если открыт, рендерит панель через per-contribution ErrorBoundary (падение оверлея → плашка,
 * app жив — цель F-8). `key` по id изолирует boundary пооверлейно (упавший оверлей не «залипает»).
 *
 * F-10d — mount-точки: outlet фильтрует реестр по `mount` (`undefined` → `'app'` по умолчанию), поэтому
 * App.tsx ставит ДВА инстанса: `<OverlayOutlet />` на уровне `.app` (8 оверлеев F-8c/F-10b/F-10c — все
 * `mount:'app'`) и `<OverlayOutlet mount="appBody" />` ВНУТРИ тела `.appBody` (там где был `.graphLayer`),
 * куда садится ТОЛЬКО `graph` (слой не покрывает хром — фикс владельца). Так вырез graph чист, а 8
 * оверлеев не сдвинулись (их точка/фильтр/ErrorBoundary/порядок прежние). См. `OverlayMount`.
 *
 * Подписка на ВЕСЬ ui-стор (`useUIStore()`): `isOpen`-селекторы оверлеев непрозрачны, поэтому outlet
 * ре-рендерится на любое ui-изменение и заново вычисляет видимость (аналог 7 прежних `useUIStore(...)`
 * подписок в App). Счастливый путь — 0 лишних DOM-узлов (Fragment + ErrorBoundary без обёртки), якоря
 * панелей не смещаются относительно прежнего рендера App.tsx.
 *
 * НЕ импортирует ни одной фича-панели напрямую — компоненты приходят из реестра как ссылки (границы
 * модулей F-1 соблюдены; после F-10b/F-10c/F-10d все оверлеи регистрируются оверлей-модулями
 * `connector/modules/*` через `ctx.overlays` — core-overlays удалён).
 */
export function OverlayOutlet({ mount = 'app' }: { mount?: OverlayMount } = {}) {
  const { t } = useTranslation();
  const state = useUIStore();

  return (
    <>
      {overlayRegistry.list().map((overlay) => {
        // mount по умолчанию 'app' (8 оверлеев не задают поле) — фильтруем по точке текущего инстанса.
        if ((overlay.mount ?? 'app') !== mount) return null;
        if (!overlay.isOpen(state)) return null;
        const Panel = overlay.component;
        return (
          <ErrorBoundary key={overlay.id} label={t(overlay.titleKey)}>
            <Panel />
          </ErrorBoundary>
        );
      })}
    </>
  );
}
