import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { tauriApi } from '../../lib/tauri-api';
import { StartingQuestions } from './StartingQuestions';
import { clearStartingQuestionsCache } from './startingQuestionsCache';

const STATIC = ['Статик 1', 'Статик 2', 'Статик 3'];

afterEach(() => {
  vi.restoreAllMocks();
  clearStartingQuestionsCache(); // изоляция: session-кэш — модульный синглтон
});

describe('StartingQuestions (AIP-SQ)', () => {
  it('открыта заметка → динамические вопросы заменяют статику; клик → onAsk', async () => {
    const onAsk = vi.fn();
    vi.spyOn(tauriApi.suggest, 'startingQuestions').mockResolvedValue([
      'Что улучшить?',
      'С чем связать?',
    ]);
    render(<StartingQuestions center="Notes/Spec.md" staticPills={STATIC} onAsk={onAsk} />);
    const btn = await screen.findByRole('button', { name: 'Что улучшить?' });
    expect(screen.queryByRole('button', { name: 'Статик 1' })).toBeNull(); // статика вытеснена
    fireEvent.click(btn);
    expect(onAsk).toHaveBeenCalledWith('Что улучшить?');
  });

  it('пустой ответ бэка (нет модели/контента) → фолбэк на статические подсказки', async () => {
    const sq = vi.spyOn(tauriApi.suggest, 'startingQuestions').mockResolvedValue([]);
    render(<StartingQuestions center="Notes/Empty.md" staticPills={STATIC} onAsk={() => {}} />);
    await waitFor(() => expect(sq).toHaveBeenCalled());
    expect(screen.getByRole('button', { name: 'Статик 1' })).toBeInTheDocument();
  });

  it('нет активной заметки → статика, бэк не зван (экономим бюджет LLM)', () => {
    const sq = vi.spyOn(tauriApi.suggest, 'startingQuestions');
    render(<StartingQuestions center={null} staticPills={STATIC} onAsk={() => {}} />);
    expect(screen.getByRole('button', { name: 'Статик 1' })).toBeInTheDocument();
    expect(sq).not.toHaveBeenCalled();
  });

  it('гонка: смена заметки A→B (B не в кэше) не оставляет вопросы A под B', async () => {
    const sq = vi.spyOn(tauriApi.suggest, 'startingQuestions');
    sq.mockResolvedValueOnce(['Вопрос про A?']); // A — резолвится
    const { rerender } = render(
      <StartingQuestions center="Race/A.md" staticPills={STATIC} onAsk={() => {}} />,
    );
    await screen.findByRole('button', { name: 'Вопрос про A?' });

    // B — ещё грузится (отложенный промис): вопросы A не должны мелькать под B → показана статика.
    let resolveB: (v: string[]) => void = () => {};
    sq.mockReturnValueOnce(new Promise<string[]>((r) => (resolveB = r)));
    rerender(<StartingQuestions center="Race/B.md" staticPills={STATIC} onAsk={() => {}} />);
    expect(screen.queryByRole('button', { name: 'Вопрос про A?' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Статик 1' })).toBeInTheDocument();

    resolveB(['Вопрос про B?']);
    await screen.findByRole('button', { name: 'Вопрос про B?' });
  });

  it('кэш-хит: повторное открытие той же заметки не дёргает бэк снова', async () => {
    const sq = vi.spyOn(tauriApi.suggest, 'startingQuestions').mockResolvedValue(['Из кэша?']);
    const { rerender } = render(
      <StartingQuestions center="Cache/X.md" staticPills={STATIC} onAsk={() => {}} />,
    );
    await screen.findByRole('button', { name: 'Из кэша?' });
    expect(sq).toHaveBeenCalledTimes(1);

    rerender(<StartingQuestions center="Cache/Y.md" staticPills={STATIC} onAsk={() => {}} />); // Y → 2-й вызов
    rerender(<StartingQuestions center="Cache/X.md" staticPills={STATIC} onAsk={() => {}} />); // X из кэша
    await screen.findByRole('button', { name: 'Из кэша?' });
    expect(sq).toHaveBeenCalledTimes(2); // X(1) + Y(1); повтор X — без вызова
  });
});
