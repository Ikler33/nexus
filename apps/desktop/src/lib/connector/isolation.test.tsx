import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MainViewOutlet } from '../../components/workspace/MainViewOutlet';
import { useUIStore } from '../../stores/ui';
import { modules } from './module-manager';
import type { NexusModule } from './types';

/**
 * ОРАКУЛ ИЗОЛЯЦИИ (F-8, обязателен): тест-модуль регистрирует НАМЕРЕННО падающую main-вью через
 * коннектор (полная цепочка module → views-реестр → MainViewOutlet → per-contribution ErrorBoundary).
 * Ожидание: рендер приложения НЕ падает — рядом стоящий «хром» жив, а на месте вью — плашка
 * «модуль упал» + reload. Цель владельца: «ИИ правит модуль → app не падает».
 */

/** Вью, падающая на рендере (симуляция сломанного модулем кода). */
function BoomView(): never {
  throw new Error('boom from test module');
}

/** Нормальная вью-заглушка (контроль: без падения плашки нет). */
function OkView() {
  return <div data-testid="ok-view">ok-view-content</div>;
}

const boomModule: NexusModule = {
  id: 'test-boom',
  activate(ctx) {
    ctx.views.register({
      id: 'test:boom',
      titleKey: 'commands.view.home', // реальный ключ → осмысленное имя в плашке
      icon: () => null,
      order: 9001,
      component: BoomView,
      activate: () => {},
      isActive: (v) => v === 'test:boom',
    });
    ctx.views.register({
      id: 'test:ok',
      titleKey: 'commands.view.home',
      icon: () => null,
      order: 9002,
      component: OkView,
      activate: () => {},
      isActive: (v) => v === 'test:ok',
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
  useUIStore.setState({ mainView: 'home' });
  errorSpy.mockRestore();
});

describe('изоляция вклада (F-8 ErrorBoundary)', () => {
  it('падающая вью тест-модуля → app жив (хром на месте) + плашка ErrorBoundary', () => {
    useUIStore.setState({ mainView: 'test:boom' as never });

    // Рендер НЕ бросает (иначе тест упал бы здесь) — ErrorBoundary ловит падение вью.
    render(
      <div>
        <span data-testid="app-chrome">chrome-alive</span>
        <MainViewOutlet />
      </div>,
    );

    // App жив: соседний «хром» отрисован.
    expect(screen.getByTestId('app-chrome')).toBeInTheDocument();
    // Плашка изоляции на месте вью (роль alert + текст «упал» + кнопка reload).
    const plate = screen.getByRole('alert');
    expect(plate).toHaveTextContent(/упал/i);
    expect(screen.getByRole('button', { name: /перезагрузить/i })).toBeInTheDocument();
    // Падение случилось внутри boundary (React залогировал в console.error).
    expect(errorSpy).toHaveBeenCalled();
  });

  it('контроль: нормальная вью модуля рендерится БЕЗ плашки', () => {
    useUIStore.setState({ mainView: 'test:ok' as never });
    render(<MainViewOutlet />);
    expect(screen.getByTestId('ok-view')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
