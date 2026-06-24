import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { AgentView } from './AgentView';
import { tauriApi } from '../../lib/tauri-api';
import { useAgentStore, sessionStatus } from '../../stores/agent';
import { useToastStore } from '../../stores/toast';
import * as mockAgent from '../../lib/mock/agent';

/** Статус сессии (последнего хода) и runId последнего хода — из мультитёрн-ленты. */
const curStatus = () => sessionStatus(useAgentStore.getState().turns);
const lastRunId = () => useAgentStore.getState().turns.at(-1)?.runId ?? null;

/** Сброс стора агента + мок-реестра прогонов между тестами (мок — память процесса). */
function reset() {
  useAgentStore.setState({
    turns: [],
    autonomy: 'confirm',
    model: 'qwen3:35b',
    perms: { read: true, write: true, web: false },
    context: null,
    approving: false,
  });
  useToastStore.setState({ toasts: [] });
  mockAgent.__reset();
}

beforeEach(reset);
afterEach(() => {
  vi.restoreAllMocks();
});

describe('AgentView (UI-1b — фронт вкладки Агента на контракте UI-1a)', () => {
  it('пустое состояние: шапка, подсказка и композер до запуска', () => {
    render(<AgentView />);
    // Шапка (заголовок) + кнопка «Новая сессия».
    expect(screen.getByText('Castor')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Новая сессия' })).toBeInTheDocument();
    // Пустое состояние — подсказка «поручите задачу».
    expect(screen.getByText('Поручите задачу агенту')).toBeInTheDocument();
    // Композер виден и его поле доступно (прогон не идёт).
    expect(screen.getByPlaceholderText('Поручите задачу агенту…')).not.toBeDisabled();
    expect(screen.getByRole('button', { name: 'Запустить' })).toBeDisabled(); // пустой ввод
  });

  it('запуск через композер: мок-стрим наполняет ленту (ответ ассистента + шаг + changeset)', async () => {
    render(<AgentView />);
    const input = screen.getByPlaceholderText('Поручите задачу агенту…');
    fireEvent.change(input, { target: { value: 'Разбери входящие' } });
    fireEvent.click(screen.getByRole('button', { name: 'Запустить' }));

    // Задача отрисована как сообщение пользователя.
    expect(await screen.findByText('Разбери входящие')).toBeInTheDocument();

    // assistantToken-дельты склеились в ответ ассистента.
    await waitFor(() => expect(screen.getByText(/Принял задачу/)).toBeInTheDocument());

    // toolCall → шаг ленты с kind инструмента (fs.read из мока).
    await waitFor(() => expect(screen.getAllByText('fs.read').length).toBeGreaterThan(0));

    // proposal → changeset с тремя файлами + заголовок «Изменения».
    await waitFor(() => expect(screen.getByText('Изменения')).toBeInTheDocument());
    expect(screen.getByText('RMS-B2B/Идея — кэш контекста.md')).toBeInTheDocument();
  });

  it('changeset apply/reject собирает decisions[] и шлёт agent_approve', async () => {
    const approveSpy = vi.spyOn(tauriApi.agent, 'approve');
    render(<AgentView />);
    fireEvent.change(screen.getByPlaceholderText('Поручите задачу агенту…'), {
      target: { value: 'задача' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Запустить' }));

    // Ждём появления changeset (proposal) и перехода в статус ожидания решения.
    await waitFor(() => expect(screen.getByText('Изменения')).toBeInTheDocument());
    await waitFor(() => expect(curStatus()).toBe('awaiting'));

    // Per-file: применяем первый файл, отклоняем второй, третий оставляем нерешённым (→ reject на бэке).
    // Кнопки перезапрашиваем после каждого клика — решённый файл сменил кнопки на бейдж (DOM-сдвиг).
    // Per-file reject = «Отклонить изменение» (отличается от bulk «Отклонить» — иначе клик попал бы в bulk).
    fireEvent.click(screen.getAllByRole('button', { name: 'Применить' })[0]); // файл 1 → applied
    fireEvent.click(screen.getAllByRole('button', { name: 'Отклонить изменение' })[0]); // файл 2 → rejected

    // Подтверждаем changeset → собранные decisions уходят в agent_approve.
    fireEvent.click(screen.getByRole('button', { name: 'Подтвердить' }));

    await waitFor(() => expect(approveSpy).toHaveBeenCalledTimes(1));
    const [runId, decisions] = approveSpy.mock.calls[0];
    expect(runId).toBe(lastRunId());
    // decisions[] = по одному на адресуемый файл; approve=true только у применённого.
    expect(decisions).toHaveLength(3);
    const byApprove = decisions.map((d) => d.approve);
    expect(byApprove.filter(Boolean)).toHaveLength(1); // ровно один applied
    expect(byApprove.filter((a) => !a)).toHaveLength(2); // rejected + нерешённый (fail-closed)
    // Каждое решение адресовано actionId из proposal (>= 0, не -1).
    expect(decisions.every((d) => d.actionId >= 0)).toBe(true);
  });

  it('bulk «Применить все» помечает все файлы applied → все approve=true', async () => {
    const approveSpy = vi.spyOn(tauriApi.agent, 'approve');
    render(<AgentView />);
    fireEvent.change(screen.getByPlaceholderText('Поручите задачу агенту…'), {
      target: { value: 'задача' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Запустить' }));
    await waitFor(() => expect(curStatus()).toBe('awaiting'));

    fireEvent.click(screen.getByRole('button', { name: 'Применить все' }));
    fireEvent.click(screen.getByRole('button', { name: 'Подтвердить' }));

    await waitFor(() => expect(approveSpy).toHaveBeenCalledTimes(1));
    const decisions = approveSpy.mock.calls[0][1];
    expect(decisions.every((d) => d.approve)).toBe(true);
  });

  it('autonomy=auto: changeset показывает авто-бейдж, аппрув не требуется', async () => {
    useAgentStore.setState({ autonomy: 'auto' });
    render(<AgentView />);
    fireEvent.change(screen.getByPlaceholderText('Поручите задачу агенту…'), {
      target: { value: 'авто-задача' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Запустить' }));

    // Auto-режим: дифы идут без proposal — changeset наполняется, бейдж «Авто».
    await waitFor(() => expect(screen.getByText('Изменения')).toBeInTheDocument());
    expect(screen.getByText(/Авто · агент ревьюит сам/)).toBeInTheDocument();
    // Нет кнопки «Подтвердить» (auto не ждёт аппрува).
    expect(screen.queryByRole('button', { name: 'Подтвердить' })).not.toBeInTheDocument();
  });

  it('мультитёрн: второе сообщение НЕ стирает первое (регрессия «переписка исчезла»)', async () => {
    useAgentStore.setState({ autonomy: 'auto' }); // auto → ход завершается сам (final → done)
    render(<AgentView />);
    const input = () => screen.getByPlaceholderText('Поручите задачу агенту…');

    // Ход 1 — отправляем и дожидаемся завершения (композер снова доступен).
    fireEvent.change(input(), { target: { value: 'первая задача' } });
    fireEvent.click(screen.getByRole('button', { name: 'Запустить' }));
    expect(await screen.findByText('первая задача')).toBeInTheDocument();
    await waitFor(() => expect(curStatus()).toBe('done'));

    // Ход 2 — отправляем второе сообщение.
    fireEvent.change(input(), { target: { value: 'вторая задача' } });
    fireEvent.click(screen.getByRole('button', { name: 'Запустить' }));
    expect(await screen.findByText('вторая задача')).toBeInTheDocument();

    // ОБА сообщения остаются в ленте — первое НЕ стёрто (суть фикса).
    expect(screen.getByText('первая задача')).toBeInTheDocument();
    expect(screen.getByText('вторая задача')).toBeInTheDocument();
    expect(useAgentStore.getState().turns).toHaveLength(2);
  });

  // W-14: правый dock «План» = РЕАЛЬНЫЕ шаги хода (tool-вызовы), а не статичная демо-заглушка (ST-G6).
  it('W-14: план показывает реальные kind-шаги прогона, без демо-меток', () => {
    useAgentStore.setState({
      turns: [
        {
          key: 0,
          epoch: 1,
          runId: 1,
          task: 'задача',
          assistantText: '',
          steps: [
            { id: 'a', kind: 'note.create', args: '{}', result: 'ok', isError: false },
            { id: 'b', kind: 'web.search', args: '{}', result: null, isError: false },
          ],
          changeset: [],
          plan: [],
          subagents: [],
          execItems: [],
          researchReport: null,
          report: null,
          error: null,
          status: 'running',
        },
      ],
    });
    render(<AgentView />);
    // dock 'plan' открыт по умолчанию → реальные kind'ы видны.
    expect(screen.getAllByText('note.create').length).toBeGreaterThan(0);
    expect(screen.getAllByText('web.search').length).toBeGreaterThan(0);
    // Старые демо-метки заглушки исчезли.
    expect(screen.queryByText('match.projects')).toBeNull();
  });

  // W-15: окно подтверждения changeset показывает не только ±N, но и inline-дифф контента по клику.
  it('W-15: inline-дифф файла раскрывается по тогглу (proposed из note.create-args)', async () => {
    useAgentStore.setState({
      turns: [
        {
          key: 0,
          epoch: 1,
          runId: 1,
          task: 'создай заметку',
          assistantText: '',
          steps: [
            {
              id: 'w',
              kind: 'note.create',
              args: JSON.stringify({ path: 'Notes/X.md', content: 'строка раз\nстрока два' }),
              result: 'proposed',
              isError: false,
            },
          ],
          changeset: [
            { path: 'Notes/X.md', add: 2, del: 0, status: 'new', actionId: 1, decision: undefined },
          ],
          plan: [],
          subagents: [],
          execItems: [],
          researchReport: null,
          report: null,
          error: null,
          status: 'awaiting',
        },
      ],
    });
    render(<AgentView />);
    expect(screen.getByText('Notes/X.md')).toBeInTheDocument();
    // До клика контента диффа нет.
    expect(screen.queryByText('строка раз')).toBeNull();
    // Тоггл диффа → строки proposed как добавленные (новый файл → current пуст).
    fireEvent.click(screen.getByRole('button', { name: 'Показать/скрыть дифф' }));
    expect(await screen.findByText('строка раз')).toBeInTheDocument();
    expect(screen.getByText('строка два')).toBeInTheDocument();
    // Ревью W-15: новый файл → чистый add-дифф, без ложной ведущей пустой `del`-строки (символ «−»).
    expect(screen.queryByText('−')).toBeNull();
  });

  it('W-14: план пуст (честный стейт), когда ход без действий', () => {
    useAgentStore.setState({
      turns: [
        {
          key: 0,
          epoch: 1,
          runId: 1,
          task: 'вопрос',
          assistantText: 'ответ',
          steps: [],
          changeset: [],
          plan: [],
          subagents: [],
          execItems: [],
          researchReport: null,
          report: null,
          error: null,
          status: 'done',
        },
      ],
    });
    render(<AgentView />);
    expect(screen.getByText(/Шагов пока нет|No steps yet/)).toBeInTheDocument();
  });
});
