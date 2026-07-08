import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AiPanelOutlet } from './AiPanelOutlet';
import { panelRegistry } from '../../lib/connector';

/**
 * F-12: AiPanelOutlet рендерит зарегистрированную в реестре `panels` панель, ПРОКИДЫВАЯ позицию
 * (`variant`), и изолирует её падение per-contribution ErrorBoundary (падение панели → плашка «модуль
 * упал», app жив — цель F-8, как у OverlayOutlet). Пустой реестр → 0 DOM-след.
 */

/** Панель-заглушка: показывает переданную позицию (проверка проброса variant). */
function OkPanel({ variant }: { variant?: string }) {
  return <div data-testid="ok-panel">panel:{variant}</div>;
}

/** Панель, падающая на рендере (симуляция сломанного модулем кода). */
function BoomPanel(): never {
  throw new Error('boom from test panel');
}

let errorSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
});

afterEach(() => {
  panelRegistry._reset();
  errorSpy.mockRestore();
});

describe('AiPanelOutlet (F-12)', () => {
  it('рендерит зарегистрированную панель и пробрасывает variant', () => {
    panelRegistry.register({ id: 'test:chat', titleKey: 'chrome.aiPanel', component: OkPanel });
    render(<AiPanelOutlet variant="bottom" />);
    expect(screen.getByTestId('ok-panel')).toHaveTextContent('panel:bottom');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('падающая панель → app жив (хром на месте) + плашка ErrorBoundary', () => {
    panelRegistry.register({ id: 'test:boom', titleKey: 'chrome.aiPanel', component: BoomPanel });
    render(
      <div>
        <span data-testid="app-chrome">chrome-alive</span>
        <AiPanelOutlet variant="side" />
      </div>,
    );
    expect(screen.getByTestId('app-chrome')).toBeInTheDocument();
    const plate = screen.getByRole('alert');
    expect(plate).toHaveTextContent(/упал/i);
    expect(errorSpy).toHaveBeenCalled();
  });

  it('пустой реестр → 0 DOM-след', () => {
    const { container } = render(<AiPanelOutlet variant="overlay" />);
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
