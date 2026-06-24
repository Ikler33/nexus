import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { SettingsView } from './SettingsView';
import { tauriApi } from '../../lib/tauri-api';
import { commands } from '../../lib/commands';
import { registerCoreCommands } from '../../lib/commands-core';
import { usePrefsStore } from '../../stores/prefs';
import { useThemeStore } from '../../stores/theme';
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

    // Активна «Оформление» → видна подпись темы.
    expect(screen.getByText(/тема|theme/i)).toBeInTheDocument();

    // Переключаемся на «О программе» → секция меняется в ui-сторе, видны имя приложения и версия.
    fireEvent.click(screen.getByRole('button', { name: /о программе|about/i }));
    expect(useUIStore.getState().settingsSection).toBe('about');
    expect(screen.getByText(/версия|version/i)).toBeInTheDocument();
  });

  // Qasr-restructure: тема выбирается КАРТОЧКОЙ (сетка визуальных превью), а не сегмент-кнопкой.
  // Сохранён смысл старого теста: клик по теме вызывает setTheme и обновляет theme-стор.
  it('Appearance (Qasr): выбор темы карточкой меняет theme-стор', () => {
    useThemeStore.getState().setTheme('light'); // нормализуем старт
    useUIStore.setState({ settingsSection: 'appearance' });
    render(<SettingsView />);

    // Карточка «Тёмная» (dark) — кликабельна (aria-pressed-тоггл), вызывает setTheme('dark').
    const darkCard = screen.getByRole('button', { name: /^(тёмная|dark)$/i });
    expect(darkCard).toHaveAttribute('aria-pressed', 'false');
    fireEvent.click(darkCard);
    expect(useThemeStore.getState().theme).toBe('dark');
    expect(darkCard).toHaveAttribute('aria-pressed', 'true');

    useThemeStore.getState().setTheme('light'); // вернуть дефолт (стор общий на сессию теста)
  });

  // Qasr-restructure: About — центрированная колонка (BrandMark + имя/версия/vault), не dl/dt/dd.
  it('About (Qasr): показывает имя приложения и версию', () => {
    useUIStore.setState({ settingsSection: 'about' });
    render(<SettingsView />);
    expect(screen.getByText(/^orvin$/i)).toBeInTheDocument(); // app.name
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

  // Hermes-6/SYNC: блок «Автономный (серверный) агент» — тогглы agentd-флагов (autonomy/sandbox/
  // shell/public-fetch) персистятся через tauriApi.settings.setAgentFlags (вне Tauri — мок).
  it('AI-секция: блок headless-агента — autonomy «Авто» зовёт setAgentFlags и показывает consent-warn', async () => {
    const { tauriApi } = await import('../../lib/tauri-api');
    const spy = vi.spyOn(tauriApi.settings, 'setAgentFlags');
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);

    // Блок загрузился (getAiConfig) → заголовок + сегмент автономии.
    const autonomyLabel = await screen.findByText(/автономия коннектора|connector autonomy/i);
    const row = autonomyLabel.closest('section') as HTMLElement;
    const autoBtn = within(row).getByRole('button', { name: /^(авто|auto)$/i });
    expect(autoBtn).toHaveAttribute('aria-pressed', 'false');

    fireEvent.click(autoBtn);
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ agentAutonomy: 'auto' }));
    // Consent-warn появляется (оптимистично), сегмент перещёлкивается после ответа мока.
    expect(await screen.findByText(/без спроса|without asking/i)).toBeInTheDocument();
    await vi.waitFor(() => expect(autoBtn).toHaveAttribute('aria-pressed', 'true'));

    // Вернуть «Подтверждать» (мок-состояние общее на сессию теста).
    fireEvent.click(within(row).getByRole('button', { name: /подтверждать|^confirm$/i }));
    await vi.waitFor(() => expect(autoBtn).toHaveAttribute('aria-pressed', 'false'));
    spy.mockRestore();
  });

  it('AI-секция: песочница/shell — Linux-only → disabled на этой платформе (мок shellSupported=false)', async () => {
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);

    // Песочница: ряд есть, но сегмент Вкл/Выкл disabled (shellSupported=false в моке/браузере).
    const sandboxLabel = await screen.findByText(/^(os-песочница|os sandbox)$/i);
    const sandboxRow = sandboxLabel.closest('section') as HTMLElement;
    expect(within(sandboxRow).getByRole('button', { name: /^вкл|^on$/i })).toBeDisabled();

    // Shell тоже disabled (требует sandbox + Linux); виден поясняющий req-текст.
    const shellLabel = await screen.findByText(/^(выполнение команд|shell execution)$/i);
    const shellRow = shellLabel.closest('section') as HTMLElement;
    expect(within(shellRow).getByRole('button', { name: /^вкл|^on$/i })).toBeDisabled();
  });

  it('AI-секция: публичный web.fetch — тоггл зовёт setAgentFlags и показывает consent-warn', async () => {
    const { tauriApi } = await import('../../lib/tauri-api');
    const spy = vi.spyOn(tauriApi.settings, 'setAgentFlags');
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);

    const label = await screen.findByText(/^(публичный web\.fetch|public web fetch)$/i);
    const row = label.closest('section') as HTMLElement;
    fireEvent.click(within(row).getByRole('button', { name: /^вкл|^on$/i }));
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ webAllowPublicFetch: true }));
    // Consent-warn (только в нём упоминается SSRF — desc содержит «любой публичный URL», не путаем).
    expect(await screen.findByText(/ssrf/i)).toBeInTheDocument();

    // Вернуть выкл.
    fireEvent.click(within(row).getByRole('button', { name: /^выкл|^off$/i }));
    await vi.waitFor(() =>
      expect(within(row).getByRole('button', { name: /^вкл|^on$/i })).toHaveAttribute(
        'aria-pressed',
        'false',
      ),
    );
    spy.mockRestore();
  });

  // W-27: блок «Подключение» — на монтировании пингует эндпоинты (getAiConfig→testConnection),
  // кнопка «Перепроверить» гонит повторно. Реюз логики W-21 SelfCheck, но постоянный (не dev-only).
  it('AI-секция: блок «Подключение» (W-27) — пингует эндпоинты и перепроверяет по кнопке', async () => {
    const { tauriApi } = await import('../../lib/tauri-api');
    const getCfg = vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
      chat: { url: 'http://192.168.0.28:8080', model: 'qwen' },
      embedding: { url: 'http://192.168.0.28:8083', model: 'bge' },
      fast: null,
      agentAutonomy: null,
      agentActuatorEnabled: false,
      sandboxEnabled: false,
      shellEnable: false,
      webAllowPublicFetch: false,
      skillsLearningEnabled: false,
      agentSkillsDir: null,
      delegationEnabled: false,
      researchEnabled: false,
      shellSupported: false,
    });
    const testSpy = vi.spyOn(tauriApi.settings, 'testConnection').mockResolvedValue();
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);

    // Заголовок блока + автопроверка на монтировании пингует chat-эндпоинт.
    expect(await screen.findByText(/^(подключение|connection)$/i)).toBeInTheDocument();
    await vi.waitFor(() =>
      expect(testSpy).toHaveBeenCalledWith('http://192.168.0.28:8080'),
    );
    // Кнопка «Перепроверить» (distinct от per-endpoint «Проверить связь») → повторный прогон.
    testSpy.mockClear();
    fireEvent.click(screen.getByRole('button', { name: /^(перепроверить|re-check)$/i }));
    await vi.waitFor(() =>
      expect(testSpy).toHaveBeenCalledWith('http://192.168.0.28:8083'),
    );

    getCfg.mockRestore();
    testSpy.mockRestore();
  });

  // W-27: подзаголовок «Возможности» группирует agent-флаги (header-only regroup, тогглы не двигались).
  it('AI-секция: подзаголовок «Возможности» (W-27) рендерится в секции агента', async () => {
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);
    expect(await screen.findByText(/^(возможности|capabilities)$/i)).toBeInTheDocument();
  });

  // Регрессия стейл-замыкания: два тоггла РАЗНЫХ контролов в одном батче (до ре-рендера) не должны
  // затирать друг друга. Старый код (`{...flags}` из замыкания) ронял autonomy при втором клике; новый
  // (мерж patch'а от flagsRef) — нет. native .click() внутри act() батчит оба до flush.
  it('AI-секция: быстрые тоггл autonomy+public-fetch в одном батче — оба сохраняются (ref, не стейл)', async () => {
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);

    const autonomyLabel = await screen.findByText(/автономия коннектора|connector autonomy/i);
    const autoBtn = within(autonomyLabel.closest('section') as HTMLElement).getByRole('button', {
      name: /^(авто|auto)$/i,
    });
    const pfLabel = await screen.findByText(/^(публичный web\.fetch|public web fetch)$/i);
    const pfOn = within(pfLabel.closest('section') as HTMLElement).getByRole('button', {
      name: /^вкл|^on$/i,
    });

    // Оба клика в одном act-батче (без промежуточного ре-рендера) — воспроизводит стейл-сценарий.
    act(() => {
      autoBtn.click();
      pfOn.click();
    });

    // autonomy=auto НЕ затёрто вторым кликом, public-fetch=on тоже применился.
    await vi.waitFor(() => expect(autoBtn).toHaveAttribute('aria-pressed', 'true'));
    expect(pfOn).toHaveAttribute('aria-pressed', 'true');

    // Вернуть дефолты (мок-состояние общее на сессию теста).
    const { tauriApi } = await import('../../lib/tauri-api');
    await tauriApi.settings.setAgentFlags({
      agentAutonomy: 'confirm',
      agentActuatorEnabled: false,
      sandboxEnabled: false,
      shellEnable: false,
      webAllowPublicFetch: false,
      skillsLearningEnabled: false,
      agentSkillsDir: null,
      delegationEnabled: false,
      researchEnabled: false,
    });
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

  // audit B10: раздел получил focus-trap → Esc закрывает модалку (а не «проваливается» в reading-mode).
  it('Esc закрывает раздел настроек (focus-trap, audit B10)', () => {
    useUIStore.setState({ tweaksOpen: true, settingsSection: 'general' });
    render(<SettingsView />);
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(useUIStore.getState().tweaksOpen).toBe(false);
  });

  // W-10 (#SL): AI-секция показывает список авто-навыков + pin зовёт команду.
  it('Самообучение: список навыков + закрепление (W-10)', async () => {
    useUIStore.setState({ settingsSection: 'ai' });
    render(<SettingsView />);
    // Навыки из мока (agent + vendor) видны.
    expect(await screen.findByText('summarize-pr')).toBeInTheDocument();
    expect(screen.getByText('obsidian-markdown')).toBeInTheDocument();
    // «Закрепить» (только у agent-навыка) зовёт setSkillPinned.
    const pinSpy = vi.spyOn(tauriApi.agent, 'setSkillPinned');
    fireEvent.click(screen.getByRole('button', { name: /^Закрепить$|^Pin$/i }));
    await vi.waitFor(() => expect(pinSpy).toHaveBeenCalledWith('summarize-pr', true));
    pinSpy.mockRestore();
  });

  // W-9 (#59): секция «Данные» — экспорт зовёт backup.exportToFile; импорт показывает отчёт.
  it('Данные: экспорт зовёт backup.exportToFile', async () => {
    useUIStore.setState({ settingsSection: 'data' });
    const exportSpy = vi
      .spyOn(tauriApi.backup, 'exportToFile')
      .mockResolvedValue('/tmp/orvin-backup.json');
    render(<SettingsView />);
    fireEvent.click(screen.getByRole('button', { name: /Экспорт в файл|Export to file/i }));
    await screen.findByText(/Сохранено в|Saved to/i);
    expect(exportSpy).toHaveBeenCalled();
    exportSpy.mockRestore();
  });

  it('Данные: импорт показывает отчёт (добавлено/пропущено)', async () => {
    useUIStore.setState({ settingsSection: 'data' });
    vi.spyOn(tauriApi.backup, 'importFromFile').mockResolvedValue({
      factsAdded: 3,
      factsSkipped: 1,
      sessionsAdded: 2,
      sessionsReused: 0,
      messagesAdded: 8,
      messagesSkipped: 0,
      episodesAdded: 1,
      episodesSkipped: 0,
      skillsAdded: 0,
      skillsSkipped: 0,
      messagesOrphaned: 0,
      episodesOrphaned: 0,
      schemaVersionMismatch: false,
    });
    render(<SettingsView />);
    fireEvent.click(screen.getByRole('button', { name: /Импорт из файла|Import from file/i }));
    expect(await screen.findByText(/Импорт завершён|Import complete/i)).toBeInTheDocument();
    expect(screen.getByText(/\+3 добавлено|\+3 added/i)).toBeInTheDocument();
  });
});
