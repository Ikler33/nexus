import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';

import * as mockPlugins from '../../lib/mock/plugins';
import { PluginsPanel } from './PluginsPanel';

beforeEach(() => {
  mockPlugins.__resetForTests(); // изоляция durable-мок-журнала между тестами
  try {
    localStorage.removeItem('nexus.plugin.consent.v1');
  } catch {
    /* jsdom без localStorage */
  }
});

describe('PluginsPanel (QASR-views, макет plugins.jsx)', () => {
  // Левый нав: вкладки «Установленные» + «Журнал доступа» + privacy-нота.
  it('left-nav: вкладки installed/audit и privacy-нота', async () => {
    render(<PluginsPanel />);
    // Карточка установленного плагина рендерится в активной вкладке installed.
    expect(await screen.findByText('Hello Reader (demo)')).toBeInTheDocument();
    // Нав-вкладки.
    expect(screen.getByRole('button', { name: /установленные|installed/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /журнал доступа|access log/i })).toBeInTheDocument();
    // Privacy-нота с щитом.
    expect(
      screen.getByText(/не получают доступ|get no network or file access/i),
    ).toBeInTheDocument();
  });

  // 3-частная карточка: имя/версия + sandbox-бейдж + чипы прав по уровням риска.
  it('карточка: версия, sandbox-бейдж и perm-чипы safe/caution/sensitive', async () => {
    render(<PluginsPanel />);
    expect(await screen.findByText('Hello Reader (demo)')).toBeInTheDocument();
    expect(screen.getByText('v0.1.0')).toBeInTheDocument();
    // Sandbox-бейдж на name-line (все плагины песочничные).
    expect(screen.getAllByText(/^sandbox$|^песочница$/i).length).toBeGreaterThan(0);
    expect(screen.getByText(/чтение заметок|read notes/i)).toBeInTheDocument(); // safe
    expect(screen.getByText(/запись заметок|write notes/i)).toBeInTheDocument(); // caution
    // sensitive net-чип: текст «Доступ к сети» встречается и в privacy-ноте → берём именно чип
    // по его title (detail = allowlist хостов мок-манифеста).
    const netChip = screen.getByTitle('api.github.com');
    expect(netChip).toHaveTextContent(/доступ к сети|network access/i);
  });

  // Не-safe права → consent-sheet перед запуском; Allow монтирует песочницу (iframe плагина).
  it('запуск с не-safe правами идёт через consent-sheet, затем песочница', async () => {
    render(<PluginsPanel />);
    fireEvent.click(await screen.findByRole('button', { name: /запустить|launch/i }));

    // Sheet: запрос прав + revocable-note.
    expect(await screen.findByText(/запрашивает права|requests permissions/i)).toBeInTheDocument();
    expect(screen.getByText(/можно отозвать|revocable/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /разрешить|^allow$/i }));
    // Песочница смонтирована: появилась кнопка «Назад» и iframe плагина.
    expect(await screen.findByRole('button', { name: /назад|^back$/i })).toBeInTheDocument();
    expect(document.querySelector('iframe')).toBeInTheDocument();
  });

  // Журнал доступа — отдельная нав-вкладка (те же данные брокер-вызовов, новое размещение).
  it('журнал доступа открывается в своей нав-вкладке', async () => {
    render(<PluginsPanel />);
    await screen.findByText('Hello Reader (demo)');
    fireEvent.click(screen.getByRole('button', { name: /журнал доступа|access log/i }));
    // До взаимодействия журнал пуст, но подпись/empty-стейт виден.
    expect(
      await screen.findByText(/каждый брокер-вызов|every plugin broker call/i),
    ).toBeInTheDocument();
  });

  // PLUG-1: журнал доступа читает DURABLE-историю (list_plugin_audit), а не только in-session вызовы —
  // предзаполненные записи (пережившие «рестарт») видны при открытии вкладки.
  it('журнал доступа показывает durable-записи (allow+deny) из list_plugin_audit', async () => {
    // Сеем durable-мок-журнал как будто вызовы были в прошлой сессии (пережили рестарт).
    const token = await mockPlugins.openSession('hello');
    await mockPlugins.invoke(token, 'vault.readFile', 'Projects/Roadmap.md'); // allow
    await mockPlugins
      .invoke(token, 'vault.writeFile', 'README.md', 'x')
      .catch(() => undefined); // deny (вне scope)

    const { container } = render(<PluginsPanel />);
    await screen.findByText('Hello Reader (demo)');
    fireEvent.click(screen.getByRole('button', { name: /журнал доступа|access log/i }));

    // Durable-записи видны: метод allow-вызова и цель deny-вызова + причина отказа.
    expect(await screen.findByText('vault.readFile')).toBeInTheDocument();
    expect(screen.getByText('README.md')).toBeInTheDocument();
    expect(screen.getByText(/vault:write/i)).toBeInTheDocument(); // текст причины отказа

    // Метка времени durable-записи рендерится (PLUG-1 Fix 3): <time> с ISO-dateTime на каждую запись.
    const times = container.querySelectorAll('time[datetime]');
    expect(times.length).toBe(2); // по одной на allow+deny
    expect(times[0].getAttribute('datetime')).toMatch(/^\d{4}-\d{2}-\d{2}T/);
    expect(times[0].textContent?.trim().length ?? 0).toBeGreaterThan(0);
  });
});
