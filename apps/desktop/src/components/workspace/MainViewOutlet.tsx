import { Suspense } from 'react';
import { useTranslation } from 'react-i18next';
import { viewRegistry } from '../../lib/connector';
import { selectMainView, useUIStore } from '../../stores/ui';
import { ErrorBoundary } from '../common/ErrorBoundary';

/**
 * App-lookup активной main-вью через реестр `views` коннектора (F-8, легализация mainView-enum F-4):
 * заменяет прежний приоритетный тернарник App.tsx. Активная вью резолвится по `mainView` (дефолт —
 * редактор), рендерится через per-contribution ErrorBoundary (падение вью → плашка, app жив). `key`
 * по id сбрасывает boundary при смене вью (упавшая вью не «залипает» после навигации).
 *
 * НЕ импортирует ни одной фича-вью напрямую — компоненты приходят из реестра как ссылки (границы
 * модулей F-1 соблюдены; вью регистрируются в lib/connector/core-views).
 */
export function MainViewOutlet() {
  const { t } = useTranslation();
  const mainView = useUIStore(selectMainView);
  const view = viewRegistry.get(mainView) ?? viewRegistry.get('editor');
  if (!view) return null; // редактор всегда зарегистрирован → недостижимо (страховка)

  const View = view.component;
  const content = view.suspense ? (
    <Suspense fallback={null}>
      <View />
    </Suspense>
  ) : (
    <View />
  );

  return (
    <ErrorBoundary key={view.id} label={t(view.titleKey)}>
      {content}
    </ErrorBoundary>
  );
}
