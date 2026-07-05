import { useTranslation } from 'react-i18next';
import { overlayRegistry } from '../../lib/connector';
import { useUIStore } from '../../stores/ui';
import { ErrorBoundary } from '../common/ErrorBoundary';

/**
 * Рендер открытых оверлеев через реестр `overlays` коннектора (F-8c, легализация 7 хардкод-строк
 * App.tsx `{xOpen && <Panel/>}`): для каждого зарегистрированного оверлея вычисляет `isOpen(state)`
 * и, если открыт, рендерит панель через per-contribution ErrorBoundary (падение оверлея → плашка,
 * app жив — цель F-8). `key` по id изолирует boundary пооверлейно (упавший оверлей не «залипает»).
 *
 * Подписка на ВЕСЬ ui-стор (`useUIStore()`): `isOpen`-селекторы оверлеев непрозрачны, поэтому outlet
 * ре-рендерится на любое ui-изменение и заново вычисляет видимость (аналог 7 прежних `useUIStore(...)`
 * подписок в App). Счастливый путь — 0 лишних DOM-узлов (Fragment + ErrorBoundary без обёртки), якоря
 * панелей не смещаются относительно прежнего рендера App.tsx.
 *
 * НЕ импортирует ни одной фича-панели напрямую — компоненты приходят из реестра как ссылки (границы
 * модулей F-1 соблюдены; оверлеи регистрируются в lib/connector/core-overlays, вырезание — F-10b).
 */
export function OverlayOutlet() {
  const { t } = useTranslation();
  const state = useUIStore();

  return (
    <>
      {overlayRegistry.list().map((overlay) => {
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
