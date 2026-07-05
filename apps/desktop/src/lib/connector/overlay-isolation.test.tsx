import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { OverlayOutlet } from '../../components/workspace/OverlayOutlet';
import { useUIStore } from '../../stores/ui';
import { modules } from './module-manager';
import type { NexusModule } from './types';

/**
 * ОРАКУЛ ИЗОЛЯЦИИ ОВЕРЛЕЯ (F-8c, обязателен): тест-модуль регистрирует НАМЕРЕННО падающий оверлей
 * через `ctx.overlays` (полная цепочка module → overlays-реестр → OverlayOutlet → per-contribution
 * ErrorBoundary — та же, что вырежет F-10b). Ожидание: рендер приложения НЕ падает — соседний «хром»
 * жив, а на месте оверлея — плашка «модуль упал» + reload. Цель владельца: «ИИ правит модуль → app
 * не падает». Регистрация через `ctx.overlays` заодно доказывает готовность контекста к F-10b.
 *
 * Видимость тест-оверлеев привязана к `graphOpen`/`syncOpen` — реальным ui-булям, которые НЕ читает
 * ни один ядровой оверлей (те при дефолтных `*Open=false` не рендерятся, не таща реальные панели).
 */

/** Оверлей, падающий на рендере (симуляция сломанного модулем кода). */
function BoomOverlay(): never {
  throw new Error('boom from test overlay');
}

/** Нормальный оверлей-заглушка (контроль: без падения плашки нет). */
function OkOverlay() {
  return <div data-testid="ok-overlay">ok-overlay-content</div>;
}

const boomModule: NexusModule = {
  id: 'test-boom-overlay',
  activate(ctx) {
    ctx.overlays.register({
      id: 'test:boom-overlay',
      titleKey: 'commands.view.goals', // реальный ключ → осмысленное имя в плашке
      order: 9001,
      isOpen: (s) => s.graphOpen, // тест-триггер (не ядровой оверлей-флаг)
      component: BoomOverlay,
    });
    ctx.overlays.register({
      id: 'test:ok-overlay',
      titleKey: 'commands.view.goals',
      order: 9002,
      isOpen: (s) => s.syncOpen, // тест-триггер
      component: OkOverlay,
    });
  },
};

let errorSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  // React логирует пойманную boundary-ошибку в console.error — глушим шум теста.
  errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
  modules.register(boomModule);
  modules.activateAll();
});

afterEach(() => {
  modules._reset();
  // Гасим тест-триггеры (и на всякий — все ядровые оверлей-флаги), чтобы не течь между тестами.
  useUIStore.setState({
    graphOpen: false,
    syncOpen: false,
    goalsOpen: false,
    memoryOpen: false,
    episodesOpen: false,
    tasksOpen: false,
    inboxOpen: false,
    digestOpen: false,
    contradictionsOpen: false,
  });
  errorSpy.mockRestore();
});

describe('изоляция оверлея (F-8c ErrorBoundary)', () => {
  it('падающий оверлей тест-модуля → app жив (хром на месте) + плашка ErrorBoundary', () => {
    useUIStore.setState({ graphOpen: true }); // открывает boom-оверлей (isOpen)

    // Рендер НЕ бросает (иначе тест упал бы здесь) — ErrorBoundary ловит падение оверлея.
    render(
      <div>
        <span data-testid="app-chrome">chrome-alive</span>
        <OverlayOutlet />
      </div>,
    );

    // App жив: соседний «хром» отрисован.
    expect(screen.getByTestId('app-chrome')).toBeInTheDocument();
    // Плашка изоляции на месте оверлея (роль alert + текст «упал» + кнопка reload).
    const plate = screen.getByRole('alert');
    expect(plate).toHaveTextContent(/упал/i);
    expect(screen.getByRole('button', { name: /перезагрузить/i })).toBeInTheDocument();
    // Падение случилось внутри boundary (React залогировал в console.error).
    expect(errorSpy).toHaveBeenCalled();
  });

  it('контроль: нормальный оверлей модуля рендерится БЕЗ плашки', () => {
    useUIStore.setState({ syncOpen: true }); // открывает ok-оверлей, boom закрыт
    render(<OverlayOutlet />);
    expect(screen.getByTestId('ok-overlay')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('закрытый оверлей (isOpen=false) не рендерится — 0 DOM-след', () => {
    // Ни один триггер не взведён → ни boom, ни ok, ни ядровые оверлеи не в DOM.
    render(<OverlayOutlet />);
    expect(screen.queryByTestId('ok-overlay')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
