import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { OverlayOutlet } from '../../components/workspace/OverlayOutlet';
import { useUIStore } from '../../stores/ui';
import { modules } from './module-manager';
import type { NexusModule } from './types';

/**
 * F-10d — mount-роутинг оверлеев (поле `mount` `OverlayContribution`, МИНИМАЛЬНО app|appBody). Тест-модуль
 * регистрирует ДВА оверлея с ОДИНАКОВЫМ `isOpen` (оба открыты) — различие только в `mount`. Доказываем:
 * `<OverlayOutlet />` (default mount:'app') рендерит ТОЛЬКО app-оверлей (поле не задано → 'app'), а
 * `<OverlayOutlet mount="appBody" />` — ТОЛЬКО appBody-оверлей. Это и есть механизм, по которому 8 прежних
 * оверлеев (без поля mount) остаются на уровне `.app`, а graph (mount:'appBody') садится ВНУТРЬ `.appBody`.
 */

function AppOverlay() {
  return <div data-testid="app-overlay">app-overlay</div>;
}
function BodyOverlay() {
  return <div data-testid="body-overlay">body-overlay</div>;
}

const mountModule: NexusModule = {
  id: 'test-mount-overlays',
  activate(ctx) {
    // Оверлей БЕЗ поля mount → default 'app' (как 8 прежних оверлеев F-10b/F-10c).
    ctx.overlays.register({
      id: 'test:app-overlay',
      titleKey: 'commands.view.goals',
      order: 9101,
      isOpen: (s) => s.graphOpen,
      component: AppOverlay,
    });
    // Оверлей mount:'appBody' (как graph F-10d).
    ctx.overlays.register({
      id: 'test:body-overlay',
      titleKey: 'commands.view.graph',
      order: 9102,
      isOpen: (s) => s.graphOpen,
      component: BodyOverlay,
      mount: 'appBody',
    });
  },
};

beforeEach(() => {
  modules.register(mountModule);
  modules.activateAll();
  useUIStore.setState({ graphOpen: true }); // оба тест-оверлея «открыты» → фильтрует только mount
});

afterEach(() => {
  modules._reset();
  useUIStore.setState({ graphOpen: false });
});

describe('mount-роутинг OverlayOutlet (F-10d)', () => {
  it('default-инстанс (mount:app) рендерит ТОЛЬКО оверлей без mount, НЕ appBody-оверлей', () => {
    render(<OverlayOutlet />);
    expect(screen.getByTestId('app-overlay')).toBeInTheDocument();
    expect(screen.queryByTestId('body-overlay')).not.toBeInTheDocument();
  });

  it('appBody-инстанс рендерит ТОЛЬКО mount:appBody-оверлей, НЕ app-оверлей', () => {
    render(<OverlayOutlet mount="appBody" />);
    expect(screen.getByTestId('body-overlay')).toBeInTheDocument();
    expect(screen.queryByTestId('app-overlay')).not.toBeInTheDocument();
  });

  it('оба инстанса вместе покрывают обе точки без дублей (app→app-оверлей, appBody→body-оверлей)', () => {
    render(
      <>
        <OverlayOutlet />
        <OverlayOutlet mount="appBody" />
      </>,
    );
    expect(screen.getAllByTestId('app-overlay')).toHaveLength(1);
    expect(screen.getAllByTestId('body-overlay')).toHaveLength(1);
  });
});
