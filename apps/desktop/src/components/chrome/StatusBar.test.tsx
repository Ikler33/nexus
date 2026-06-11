import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { useJobsStore } from '../../stores/jobs';
import { useSyncStore } from '../../stores/sync';
import { useUIStore } from '../../stores/ui';
import { StatusBar } from './StatusBar';

afterEach(() => {
  vi.restoreAllMocks();
  useJobsStore.setState({ counts: { pending: 0, running: 0, dead: 0 } });
  useSyncStore.setState({ mergeRequired: false, conflictFiles: null });
  useUIStore.setState({ conflictOpen: false });
});

describe('StatusBar — индикатор задач (ADR-007 срез 5 / DP-4)', () => {
  it('занятый планировщик → прогресс «N задач»; ошибки — отдельным бейджем', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 1, pending: 2, dead: 1 });
    render(<StatusBar />);
    await waitFor(() => expect(screen.getByText(/3 задач|3 tasks/)).toBeInTheDocument());
    expect(screen.getByText(/⚠ 1/)).toBeInTheDocument();
  });

  it('пустая очередь → индикатора нет; right-блок Local/UTF-8/Markdown на месте', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 0 });
    render(<StatusBar />);
    await waitFor(() => expect(tauriApi.scheduler.counts).toHaveBeenCalled());
    expect(screen.queryByText(/задач|tasks|⚠/)).toBeNull();
    expect(screen.getByText(/локально|local/i)).toBeInTheDocument();
    expect(screen.getByText('UTF-8')).toBeInTheDocument();
    expect(screen.getByText('Markdown')).toBeInTheDocument();
  });

  // ── DP-14 (макет app.jsx StatusBar): synced/изменения слева, «Проиндексировано · N», пилюля ──

  it('чистое дерево → «Синхронизировано»; пустая очередь → «Проиндексировано · N»', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 0 });
    vi.spyOn(tauriApi.git, 'status').mockResolvedValue([]);
    vi.spyOn(tauriApi.vault, 'notesCount').mockResolvedValue(42);
    render(<StatusBar />);
    await waitFor(() =>
      expect(screen.getByText(/Синхронизировано|Synced/)).toBeInTheDocument(),
    );
    expect(screen.getByText(/Проиндексировано|Indexed/)).toBeInTheDocument();
    expect(screen.getByText(/· 42/)).toBeInTheDocument();
  });

  it('правки в дереве → «Изменения · N»; merge-required → конфликт-пилюля открывает резолвер', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 0 });
    vi.spyOn(tauriApi.git, 'status').mockResolvedValue([
      { path: 'a.md', kind: 'modified' },
      { path: 'b.md', kind: 'new' },
    ]);
    vi.spyOn(tauriApi.vault, 'notesCount').mockResolvedValue(7);
    useSyncStore.setState({ mergeRequired: true, conflictFiles: 2 });
    render(<StatusBar />);
    await waitFor(() => expect(screen.getByText(/Изменения · 2|Changes · 2/)).toBeInTheDocument());
    const pill = screen.getByRole('button', { name: /2 конфликта|2 conflicts/ });
    fireEvent.click(pill);
    expect(useUIStore.getState().conflictOpen).toBe(true);
  });

  // ── Модалка деталей dead-джоб за «⚠ N» (отчёт владельца 2026-06-11: ошибки нечем посмотреть) ──

  it('клик по «⚠ N» открывает модалку: kind по-человечески, текст ошибки, «Повторить» зовёт retry', async () => {
    const now = Math.floor(Date.now() / 1000);
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 2 });
    vi.spyOn(tauriApi.scheduler, 'deadJobs').mockResolvedValue([
      { id: 7, kind: 'newsfeed', attempts: 3, lastError: 'HTTP 404: нет связи с моделью', updatedAt: now - 120 },
      { id: 8, kind: 'home_widget:context_drift', attempts: 2, lastError: null, updatedAt: now - 600 },
    ]);
    const retry = vi.spyOn(tauriApi.scheduler, 'retryDead').mockResolvedValue(true);

    render(<StatusBar />);
    fireEvent.click(await screen.findByRole('button', { name: /⚠ 2/ }));

    // Известный kind — человеческое имя; home_widget:* — по префиксу; ошибка — как есть.
    expect(await screen.findByText(/Лента новостей|News feed/)).toBeInTheDocument();
    expect(screen.getByText(/HTTP 404: нет связи с моделью/)).toBeInTheDocument();
    expect(screen.getByText(/(Виджет Home|Home widget) · context_drift/)).toBeInTheDocument();

    fireEvent.click(screen.getAllByRole('button', { name: /Повторить|Retry/ })[0]);
    await waitFor(() => expect(retry).toHaveBeenCalledWith(7));
  });

  it('«Очистить все» зовёт clearDead → пустое состояние модалки', async () => {
    const now = Math.floor(Date.now() / 1000);
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 1 });
    vi.spyOn(tauriApi.scheduler, 'deadJobs')
      .mockResolvedValueOnce([
        { id: 9, kind: 'digest', attempts: 5, lastError: 'таймаут', updatedAt: now - 60 },
      ])
      .mockResolvedValue([]);
    const clear = vi.spyOn(tauriApi.scheduler, 'clearDead').mockResolvedValue(1);

    render(<StatusBar />);
    fireEvent.click(await screen.findByRole('button', { name: /⚠ 1/ }));
    fireEvent.click(await screen.findByRole('button', { name: /Очистить все|Clear all/ }));

    await waitFor(() => expect(clear).toHaveBeenCalled());
    expect(await screen.findByText(/Ошибок нет|No errors/)).toBeInTheDocument();
  });

  it('прогресс скана (vault:index-progress): реальный бар «Индексация N/M», финиш гасит', async () => {
    vi.spyOn(tauriApi.scheduler, 'counts').mockResolvedValue({ running: 0, pending: 0, dead: 0 });
    vi.spyOn(tauriApi.vault, 'notesCount').mockResolvedValue(10);
    let emit: (p: { done: number; total: number }) => void = () => {};
    vi.spyOn(tauriApi.events, 'onIndexProgress').mockImplementation(async (cb) => {
      emit = cb;
      return () => {};
    });
    render(<StatusBar />);
    await waitFor(() => expect(tauriApi.events.onIndexProgress).toHaveBeenCalled());

    act(() => emit({ done: 20, total: 100 }));
    expect(await screen.findByText(/Индексация 20\/100|Indexing 20\/100/)).toBeInTheDocument();

    act(() => emit({ done: 100, total: 100 }));
    await waitFor(() =>
      expect(screen.queryByText(/Индексация|Indexing/)).toBeNull(),
    );
  });
});
