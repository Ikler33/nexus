import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';

import { PluginsPanel } from './PluginsPanel';

beforeEach(() => {
  try {
    localStorage.removeItem('nexus.plugin.consent.v1');
  } catch {
    /* jsdom без localStorage */
  }
});

describe('PluginsPanel (DP-8, макет plugins.jsx)', () => {
  // Карточка установленного плагина: имя/версия + чипы прав по уровням риска.
  it('карточка с perm-чипами уровней safe/caution/sensitive', async () => {
    render(<PluginsPanel />);
    expect(await screen.findByText('Hello Reader (demo)')).toBeInTheDocument();
    expect(screen.getByText('v0.1.0')).toBeInTheDocument();
    expect(screen.getByText(/чтение заметок|read notes/i)).toBeInTheDocument(); // safe
    expect(screen.getByText(/запись заметок|write notes/i)).toBeInTheDocument(); // caution
    expect(screen.getByText(/доступ к сети|network access/i)).toBeInTheDocument(); // sensitive
  });

  // Не-safe права → consent-sheet перед запуском; Allow открывает песочницу и персистит решение.
  it('запуск с не-safe правами идёт через consent-sheet', async () => {
    render(<PluginsPanel />);
    fireEvent.click(await screen.findByRole('button', { name: /запустить|launch/i }));

    // Sheet: строки прав с описаниями + revocable-note.
    expect(await screen.findByText(/запрашивает права|requests permissions/i)).toBeInTheDocument();
    expect(screen.getByText(/можно отозвать|revocable/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /разрешить|^allow$/i }));
    // Песочница открылась (журнал брокера в сайдбаре вкладки).
    expect(await screen.findByText(/вызовы брокера|broker calls/i)).toBeInTheDocument();
  });
});
