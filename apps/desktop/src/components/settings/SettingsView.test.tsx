import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { SettingsView } from './SettingsView';
import { commands } from '../../lib/commands';
import { registerCoreCommands } from '../../lib/commands-core';
import { usePrefsStore } from '../../stores/prefs';
import { useUIStore } from '../../stores/ui';

describe('SettingsView (кросс-план #11, оболочка раздела)', () => {
  it('рендерит нав секций и переключает их', () => {
    useUIStore.setState({ settingsSection: 'appearance' });
    render(<SettingsView />);

    // Левый нав — секции (вкл. новые «Основное»/«Редактор», слайс 3).
    expect(screen.getByRole('button', { name: /основное|general/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /редактор|editor/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /оформление|appearance/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /модели|models/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /горячие|hotkeys/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /о программе|about/i })).toBeInTheDocument();

    // Активна «Оформление» → видны контролы темы.
    expect(screen.getByText(/тема|theme/i)).toBeInTheDocument();

    // Переключаемся на «О программе» → секция меняется в ui-сторе и видна версия/vault.
    fireEvent.click(screen.getByRole('button', { name: /о программе|about/i }));
    expect(useUIStore.getState().settingsSection).toBe('about');
    expect(screen.getByText(/версия|version/i)).toBeInTheDocument();
  });

  it('AI-секция (слайс 2): рендерит форму, проверяет связь и сохраняет', async () => {
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);

    // Три эндпоинта: чат + эмбеддинги + быстрая модель (примитивы).
    expect(screen.getByText(/^(чат-модель|chat model)$/i)).toBeInTheDocument();
    expect(screen.getByText(/эмбеддинг|embedding/i)).toBeInTheDocument();
    expect(screen.getByText(/быстрая модель|fast model/i)).toBeInTheDocument();
    const urls = screen.getAllByPlaceholderText(/127\.0\.0\.1:8080/);
    expect(urls).toHaveLength(3);

    // Ввести chat URL и проверить связь → бейдж «Доступен» (мок резолвит валидный URL).
    fireEvent.change(urls[0], { target: { value: 'http://192.168.0.172:8080' } });
    fireEvent.click(screen.getAllByRole('button', { name: /проверить|test connection/i })[0]);
    expect(await screen.findByText(/доступен|reachable/i)).toBeInTheDocument();

    // Сохранить → подтверждение (embedding не менялся → без требования перезапуска).
    fireEvent.click(screen.getByRole('button', { name: /^сохранить$|^save$/i }));
    expect(await screen.findByText(/сохранено|saved/i)).toBeInTheDocument();
  });

  // Срез 2 net.md: тоггл «офлайн» (E2) + per-feature opt-in (E6) применяются мгновенно через
  // tauriApi.egress (вне Tauri — стейтфул-мок) и отражаются в aria-pressed сегмента Вкл/Выкл.
  it('AI-секция: блок «Сеть (egress)» — офлайн-тоггл зовёт API и обновляет состояние', async () => {
    const { tauriApi } = await import('../../lib/tauri-api');
    const spy = vi.spyOn(tauriApi.egress, 'setOffline');
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);

    // Блок загрузился (getState) → строка «Офлайн-режим» с сегментом Вкл/Выкл.
    const offlineLabel = await screen.findByText(/офлайн-режим|offline mode/i);
    const row = offlineLabel.closest('section') as HTMLElement;
    const onBtn = within(row).getByRole('button', { name: /^вкл|^on$/i });
    expect(onBtn).toHaveAttribute('aria-pressed', 'false');

    fireEvent.click(onBtn);
    expect(spy).toHaveBeenCalledWith(true);
    // Мок вернул свежий снимок → сегмент перещёлкнулся.
    await vi.waitFor(() => expect(onBtn).toHaveAttribute('aria-pressed', 'true'));

    // Вернуть обратно (мок-состояние общее на сессию теста).
    fireEvent.click(within(row).getByRole('button', { name: /^выкл|^off$/i }));
    await vi.waitFor(() => expect(onBtn).toHaveAttribute('aria-pressed', 'false'));
    spy.mockRestore();
  });

  it('AI-секция: пустой URL → «Недоступен»; смена эмбеддинга → требование перезапуска', async () => {
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);
    const urls = screen.getAllByPlaceholderText(/127\.0\.0\.1:8080/);
    const tests = screen.getAllByRole('button', { name: /проверить|test connection/i });

    // Проверка связи embedding-эндпоинта без URL (пробелы → пусто после trim) → бейдж «Недоступен».
    fireEvent.change(urls[1], { target: { value: '   ' } });
    fireEvent.click(tests[1]);
    expect(await screen.findByText(/недоступен|unreachable/i)).toBeInTheDocument();

    // Задать новый embedding URL и сохранить → требование перезапуска (эмбеддинг изменился).
    fireEvent.change(urls[1], { target: { value: 'http://127.0.0.1:8083' } });
    fireEvent.click(screen.getByRole('button', { name: /^сохранить$|^save$/i }));
    expect(await screen.findByText(/перезапустите|restart/i)).toBeInTheDocument();
  });

  it('General (слайс 3): секция с переключателем языка RU/EN', () => {
    useUIStore.setState({ settingsSection: 'general' });
    render(<SettingsView />);
    expect(screen.getByText(/язык|language/i)).toBeInTheDocument();
    // Эндонимы языков рендерятся как есть, независимо от текущей локали.
    expect(screen.getByRole('button', { name: 'Русский' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'English' })).toBeInTheDocument();
  });

  it('Editor (слайс 3): тогл читаемой ширины меняет prefs-стор и CSS-переменную', () => {
    usePrefsStore.getState().setReadableLineWidth(true); // нормализуем старт
    useUIStore.setState({ settingsSection: 'editor' });
    render(<SettingsView />);
    expect(usePrefsStore.getState().readableLineWidth).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: /^выкл$|^off$/i }));
    expect(usePrefsStore.getState().readableLineWidth).toBe(false);
    expect(document.documentElement.style.getPropertyValue('--editor-max-width')).toBe('none');

    fireEvent.click(screen.getByRole('button', { name: /^вкл$|^on$/i }));
    expect(usePrefsStore.getState().readableLineWidth).toBe(true);
    expect(document.documentElement.style.getPropertyValue('--editor-max-width')).toBe('44rem');
  });

  it('Hotkeys (слайс 4): список команд, захват комбинации и сброс', () => {
    const reg = registerCoreCommands();
    try {
      useUIStore.setState({ settingsSection: 'hotkeys' });
      render(<SettingsView />);

      // Строка «Новая заметка» (file.new) с её дефолтным хоткеем (Ctrl+N в jsdom = не-mac).
      const row = screen.getByText(/^(новая заметка|new note)$/i).closest('li') as HTMLElement;
      expect(within(row).getByText(/ctrl\+n|⌘n/i)).toBeInTheDocument();

      // «Изменить» → захват → жмём Ctrl+Shift+N (capture-фаза window).
      fireEvent.click(within(row).getByRole('button', { name: /изменить|change/i }));
      act(() => {
        window.dispatchEvent(
          new KeyboardEvent('keydown', { key: 'N', ctrlKey: true, shiftKey: true }),
        );
      });
      expect(commands.userKeyFor('file.new')).toBe('ctrl+shift+n');

      // Появилась кнопка сброса → возвращает дефолт.
      const row2 = screen.getByText(/^(новая заметка|new note)$/i).closest('li') as HTMLElement;
      fireEvent.click(within(row2).getByRole('button', { name: /сбросить|reset/i }));
      expect(commands.userKeyFor('file.new')).toBeUndefined();
    } finally {
      reg.dispose();
      commands._reset();
    }
  });
});
