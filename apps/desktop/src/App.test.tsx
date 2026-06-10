import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from './App';
import { useUIStore } from './stores/ui';
import { useVaultStore } from './stores/vault';
import { useWorkspaceStore } from './stores/workspace';

beforeEach(() => {
  useVaultStore.setState({ info: null, childrenByPath: {}, expanded: {}, loading: {} });
  useWorkspaceStore.getState().reset();
  // onboardingDone: welcome ведёт сразу к «Открыть vault» (4-шаговый flow — отдельный тест).
  useUIStore.setState({ homeOpen: true, newsOpen: false, onboardingDone: true, onboardingActive: false });
});

describe('App (Ф0-3 / Ф4-11 / DP-1)', () => {
  it('первый запуск: онбординг → «Открыть vault» показывает файловое дерево', async () => {
    render(<App />);
    // Без vault — приветственный экран онбординга (Ф4-11).
    fireEvent.click(await screen.findByRole('button', { name: /Открыть vault/ }));
    expect(await screen.findByText('README')).toBeInTheDocument(); // DP-15: дерево без .md
    expect(screen.getByText('Projects')).toBeInTheDocument();
  });

  it('после открытия vault стартовая вью — HOME-дашборд (DP-1); файл из дерева → редактор', async () => {
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: /Открыть vault/ }));
    await screen.findByText('README');
    // Лендинг — Home: приветствие + hero-поиск.
    expect(
      screen.getByText(/добрый день|доброе утро|добрый вечер|доброй ночи|good/i),
    ).toBeInTheDocument();
    // Открытие файла из дерева закрывает Home и показывает редактор.
    fireEvent.click(screen.getByText('README'));
    expect(await screen.findByRole('tab', { name: /README/ })).toBeInTheDocument();
    expect(useUIStore.getState().homeOpen).toBe(false);
  });

  // DP-7: первый запуск — 4-шаговый онбординг (welcome → vault → AI → индексация → вход).
  it('первый запуск: онбординг шагает welcome → vault → AI → индексация → Home', async () => {
    useUIStore.setState({ onboardingDone: false });
    render(<App />);

    fireEvent.click(await screen.findByRole('button', { name: /начать настройку|get started/i }));
    fireEvent.click(await screen.findByRole('button', { name: /открыть папку|open folder/i }));

    // Шаг AI: мок-конфиг читается, идём дальше.
    fireEvent.click(await screen.findByRole('button', { name: /продолжить|continue/i }));

    // Шаг индексации: вход доступен сразу (vault уже открыт).
    fireEvent.click(await screen.findByRole('button', { name: /открыть nexus|open nexus/i }));
    expect(useUIStore.getState().onboardingDone).toBe(true);
    // Приложение вошло: Home-лендинг с приветствием.
    expect(
      await screen.findByText(/добрый день|доброе утро|добрый вечер|доброй ночи|good/i),
    ).toBeInTheDocument();
  });
});
