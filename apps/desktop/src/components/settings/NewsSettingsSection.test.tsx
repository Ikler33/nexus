import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { NewsSettingsSection } from './NewsSettingsSection';
import { tauriApi } from '../../lib/tauri-api';
import type { NewsConfig, NewsSource } from '../../lib/tauri-api';

// Базовый конфиг с НЕпустыми «прочими» полями — чтобы проверить, что правка одного поля не затирает
// остальные (МЕРЖ). model_pref=fast, два хоста, свои ключевые слова.
const BASE_CONFIG: NewsConfig = {
  enabled: true,
  sources: { willison: false }, // одно явное переопределение
  keywords: ['llm', 'rag'],
  extraHosts: ['kept.example.com'],
  modelPref: 'fast',
};

const SOURCES: NewsSource[] = [
  { id: 'openai', title: 'OpenAI', enabled: true, langRu: false },
  { id: 'willison', title: 'Simon Willison', enabled: false, langRu: false },
];

function stubLoaders() {
  vi.spyOn(tauriApi.news, 'getConfig').mockResolvedValue({
    ...BASE_CONFIG,
    sources: { ...BASE_CONFIG.sources },
  });
  vi.spyOn(tauriApi.news, 'sources').mockResolvedValue(SOURCES.map((s) => ({ ...s })));
  vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
    chat: { url: 'http://main:8080', model: 'qwen' },
    embedding: null,
    fast: { url: 'http://fast:8084', model: 'gemma' },
  } as Awaited<ReturnType<typeof tauriApi.settings.getAiConfig>>);
}

describe('NewsSettingsSection (W-40)', () => {
  beforeEach(() => {
    stubLoaders();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('рендерит источники и текущую модель (fast) из конфига', async () => {
    render(<NewsSettingsSection />);

    // Сегмент модели: «Быстрая» активна (modelPref=fast), под ней — url ai.fast.
    const fast = await screen.findByRole('button', { name: /быстрая/i });
    expect(fast).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: /основная/i })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    expect(screen.getByText('http://fast:8084')).toBeInTheDocument();

    // Источники из реестра: чекбоксы с действующими флагами (willison переопределён в OFF).
    expect(screen.getByLabelText('OpenAI')).toBeChecked();
    expect(screen.getByLabelText('Simon Willison')).not.toBeChecked();

    // Сохранённый доп. хост виден.
    expect(screen.getByText('kept.example.com')).toBeInTheDocument();
  });

  it('P1-16: consent-тоггл отражает config.enabled (Вкл активна при enabled=true)', async () => {
    render(<NewsSettingsSection />);
    // BASE_CONFIG.enabled === true → сегмент «Вкл» нажат, «Выкл» — нет.
    const on = await screen.findByRole('button', { name: /^вкл$/i });
    const off = screen.getByRole('button', { name: /^выкл$/i });
    expect(on).toHaveAttribute('aria-pressed', 'true');
    expect(off).toHaveAttribute('aria-pressed', 'false');
  });

  it('P1-16: выключение consent зовёт setConfig с enabled=false, сохраняя sources/extraHosts/keywords/model', async () => {
    const setSpy = vi
      .spyOn(tauriApi.news, 'setConfig')
      .mockImplementation((cfg) => Promise.resolve(cfg));
    render(<NewsSettingsSection />);

    fireEvent.click(await screen.findByRole('button', { name: /^выкл$/i }));

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1));
    const sent = setSpy.mock.calls[0][0];
    // ЧЕСТНО меняем реальный enabled-consent…
    expect(sent.enabled).toBe(false);
    // …МЕРЖ не теряет прочие поля (sources/extraHosts/keywords/modelPref).
    expect(sent.sources).toEqual({ willison: false });
    expect(sent.extraHosts).toEqual(['kept.example.com']);
    expect(sent.keywords).toEqual(['llm', 'rag']);
    expect(sent.modelPref).toBe('fast');
  });

  it('смена модели на «Основная» зовёт setConfig с modelPref=main БЕЗ затирания прочих полей', async () => {
    const setSpy = vi
      .spyOn(tauriApi.news, 'setConfig')
      .mockImplementation((cfg) => Promise.resolve(cfg));
    render(<NewsSettingsSection />);

    fireEvent.click(await screen.findByRole('button', { name: /основная/i }));

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1));
    const sent = setSpy.mock.calls[0][0];
    expect(sent.modelPref).toBe('main');
    // МЕРЖ: остальные поля сохранены как были.
    expect(sent.enabled).toBe(true);
    expect(sent.keywords).toEqual(['llm', 'rag']);
    expect(sent.extraHosts).toEqual(['kept.example.com']);
    expect(sent.sources).toEqual({ willison: false });
  });

  it('тоггл источника зовёт setConfig с обновлённым sources, прочие поля целы', async () => {
    const setSpy = vi
      .spyOn(tauriApi.news, 'setConfig')
      .mockImplementation((cfg) => Promise.resolve(cfg));
    render(<NewsSettingsSection />);

    // Выключаем OpenAI (был ON по дефолту реестра → переопределение в OFF).
    fireEvent.click(await screen.findByLabelText('OpenAI'));

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1));
    const sent = setSpy.mock.calls[0][0];
    expect(sent.sources).toEqual({ willison: false, openai: false });
    // МЕРЖ: модель и остальное не тронуты.
    expect(sent.modelPref).toBe('fast');
    expect(sent.extraHosts).toEqual(['kept.example.com']);
    expect(sent.keywords).toEqual(['llm', 'rag']);
  });

  it('правка ключевых слов на blur зовёт setConfig с распарсенным массивом', async () => {
    const setSpy = vi
      .spyOn(tauriApi.news, 'setConfig')
      .mockImplementation((cfg) => Promise.resolve(cfg));
    render(<NewsSettingsSection />);

    const ta = await screen.findByLabelText(/ключевые слова/i);
    fireEvent.change(ta, { target: { value: 'mcp, agents\nlocal' } });
    fireEvent.blur(ta);

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1));
    expect(setSpy.mock.calls[0][0].keywords).toEqual(['mcp', 'agents', 'local']);
  });

  it('добавление и снятие доп. хоста мержит extraHosts', async () => {
    const setSpy = vi
      .spyOn(tauriApi.news, 'setConfig')
      .mockImplementation((cfg) => Promise.resolve(cfg));
    render(<NewsSettingsSection />);

    // Добавить новый хост через поле + кнопку «Добавить».
    const input = await screen.findByLabelText(/дополнительные хосты/i);
    fireEvent.change(input, { target: { value: 'New.Example.COM' } });
    fireEvent.click(screen.getByRole('button', { name: /^добавить$/i }));

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1));
    // Нормализация в lower-case + сохранение прежнего хоста (МЕРЖ).
    expect(setSpy.mock.calls[0][0].extraHosts).toEqual(['kept.example.com', 'new.example.com']);

    // Снять прежний хост.
    const keptRow = screen.getByText('kept.example.com').closest('div')!;
    fireEvent.click(within(keptRow).getByRole('button', { name: /снять/i }));
    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(2));
    expect(setSpy.mock.calls[1][0].extraHosts).not.toContain('kept.example.com');
  });

  it('ошибка сохранения показывает тост ошибки и перечитывает конфиг', async () => {
    vi.spyOn(tauriApi.news, 'setConfig').mockRejectedValue(new Error('disk full'));
    const { useToastStore } = await import('../../stores/toast');
    const addToast = vi.spyOn(useToastStore.getState(), 'addToast');
    render(<NewsSettingsSection />);

    fireEvent.click(await screen.findByRole('button', { name: /основная/i }));

    await waitFor(() =>
      expect(addToast).toHaveBeenCalledWith(
        expect.stringMatching(/не удалось сохранить/i),
        expect.objectContaining({ kind: 'error' }),
      ),
    );
  });
});
