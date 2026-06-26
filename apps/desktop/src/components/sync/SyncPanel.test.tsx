import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { SyncPanel } from './SyncPanel';
import { __resetGitMock } from '../../lib/mock/git';
import { tauriApi } from '../../lib/tauri-api';

// Мок git мутабелен (общий module-level `dirty`) — тест «подмножество» гоняет РЕАЛЬНЫЙ commitPaths
// end-to-end и мутирует его. Сбрасываем сид перед каждым `it`, чтобы тесты не зависели от порядка.
beforeEach(() => {
  __resetGitMock();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SyncPanel commit — ошибка не глотается (audit B13)', () => {
  it('сбой git.commit показывает error-исход, а не тихо проглатывается', async () => {
    vi.spyOn(tauriApi.git, 'commit').mockRejectedValue(new Error('detached HEAD'));
    render(<SyncPanel />);

    // Кнопка коммита доступна после загрузки статуса (мок отдаёт грязные файлы).
    const btn = await screen.findByRole('button', { name: /закоммитить|^commit$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    fireEvent.click(btn);

    expect(await screen.findByText(/detached HEAD/)).toBeInTheDocument();
  });

  // audit B16: сбой setRemote/setToken в saveRemote больше не глотается пустым catch.
  it('сбой setRemote показывает ошибку, а не тихо проглатывается', async () => {
    vi.spyOn(tauriApi.git, 'setRemote').mockRejectedValue(new Error('remote refused'));
    render(<SyncPanel />);
    const urlInput = await screen.findByPlaceholderText(/github\.com\/you\/vault/i);
    fireEvent.change(urlInput, { target: { value: 'https://example.com/repo.git' } });
    fireEvent.click(screen.getByRole('button', { name: /подключить|^connect$/i }));
    expect(await screen.findByText(/remote refused/)).toBeInTheDocument();
  });
});

// P1-17: отзыв токена — кнопка видна только когда connected, клик зовёт реальный git.clearToken
// (бэкенд git_clear_token) и сбрасывает connected → false (статус «нет токена»).
describe('SyncPanel — отзыв токена (P1-17)', () => {
  it('connected → кнопка «Отозвать» видна; клик зовёт git.clearToken и connected → false', async () => {
    // На load мок hasToken=true → connected. Кнопка отзыва появляется.
    vi.spyOn(tauriApi.git, 'hasToken').mockResolvedValue(true);
    const clear = vi.spyOn(tauriApi.git, 'clearToken').mockResolvedValue(undefined);
    render(<SyncPanel />);

    // Дожидаемся индикатора «токен в keychain» (connected===true).
    await screen.findByText(/токен в keychain|token in keychain/i);
    const revoke = screen.getByRole('button', { name: /отозвать токен|revoke token/i });
    expect(revoke).toBeInTheDocument();

    fireEvent.click(revoke);
    await waitFor(() => expect(clear).toHaveBeenCalledTimes(1));

    // connected → false: индикатор сменился на «нет токена», кнопка отзыва исчезла.
    await screen.findByText(/нет токена|no token/i);
    expect(screen.queryByRole('button', { name: /отозвать токен|revoke token/i })).toBeNull();
  });

  it('НЕ connected → кнопки «Отозвать» нет', async () => {
    // Дефолтный мок: токена нет → hasToken=false → connected=false.
    render(<SyncPanel />);
    await screen.findByText(/нет токена|no token/i);
    expect(screen.queryByRole('button', { name: /отозвать токен|revoke token/i })).toBeNull();
  });
});

// P1-5: выборочный коммит — чекбоксы выбора файлов → коммит ТОЛЬКО выбранных через commitPaths.
describe('SyncPanel — выборочный коммит (P1-5)', () => {
  const commitBtn = () => screen.getByRole('button', { name: /закоммитить|выбранные|^commit/i });

  it('дефолт: все файлы выбраны → дефолтный «Коммит» зовёт commit(all), НЕ commitPaths (не регресс)', async () => {
    const commit = vi
      .spyOn(tauriApi.git, 'commit')
      .mockResolvedValue({ status: 'committed', oid: 'x', message: 'm', files: 2 });
    const commitPaths = vi.spyOn(tauriApi.git, 'commitPaths');
    render(<SyncPanel />);

    const btn = await screen.findByRole('button', { name: /закоммитить|^commit$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    // дефолт: оба чекбокса файлов отмечены
    const boxes = screen.getAllByRole('checkbox');
    boxes.forEach((b) => expect(b).toBeChecked());

    fireEvent.click(btn);
    await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
    expect(commitPaths).not.toHaveBeenCalled();
  });

  it('подмножество (снят 1 файл) → commitPaths с выбранными; РЕАЛЬНЫЙ мок оставляет невыбранный dirty', async () => {
    // Намеренно НЕ мокаем commitPaths/status — гоняем реальный мок end-to-end, чтобы тест доказывал
    // retention через настоящий контракт (мок зеркалит бэк: невыбранное остаётся), а не через стаб.
    const commitPaths = vi.spyOn(tauriApi.git, 'commitPaths');

    render(<SyncPanel />);
    const btn = await screen.findByRole('button', { name: /закоммитить|^commit$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    // оба файла видны до коммита
    expect(screen.getByText('README.md')).toBeInTheDocument();
    expect(screen.getByText('Idea.md')).toBeInTheDocument();

    // снимаем выбор с README.md (его чекбокс — по aria-label)
    const readmeBox = screen.getByRole('checkbox', { name: /README\.md/i });
    fireEvent.click(readmeBox);
    expect(readmeBox).not.toBeChecked();

    // лейбл кнопки переключился на «выбранные · N» с N = размер выбора (NIT-5)
    expect(screen.getByRole('button', { name: /выбранные · 1|selected · 1/i })).toBeInTheDocument();

    fireEvent.click(commitBtn());
    await waitFor(() => expect(commitPaths).toHaveBeenCalledTimes(1));
    // вызван ровно с выбранным путём (Notes/Idea.md), БЕЗ снятого README.md
    expect(commitPaths.mock.calls[0][0]).toEqual(['Notes/Idea.md']);

    // реальный мок закоммитил ТОЛЬКО Idea.md → он исчез, невыбранный README.md ОСТАЛСЯ dirty
    await waitFor(() => expect(screen.queryByText('Idea.md')).not.toBeInTheDocument());
    expect(screen.getByText('README.md')).toBeInTheDocument();
  });

  // Изоляция (бежит СРАЗУ после «подмножество», которое необратимо мутирует общий мок-`dirty`):
  // оба файла снова видны → доказывает, что `beforeEach(__resetGitMock)` сбросил сид, а не порядок
  // объявления тестов держит набор зелёным.
  it('изоляция: после мутирующего теста beforeEach сбрасывает мок → оба файла снова видны', async () => {
    render(<SyncPanel />);
    await screen.findByRole('button', { name: /закоммитить|^commit$/i });
    expect(screen.getByText('README.md')).toBeInTheDocument();
    expect(screen.getByText('Idea.md')).toBeInTheDocument();
  });

  it('пустой выбор (снято всё) → кнопка коммита disabled, commit не зовётся', async () => {
    const commit = vi.spyOn(tauriApi.git, 'commit');
    const commitPaths = vi.spyOn(tauriApi.git, 'commitPaths');
    render(<SyncPanel />);
    await screen.findByRole('button', { name: /закоммитить|^commit$/i });

    // мастер-чекбокс «выбрать все» снимает весь выбор
    const selectAll = screen.getByRole('checkbox', { name: /выбрать все|select all/i });
    expect(selectAll).toBeChecked();
    fireEvent.click(selectAll);
    expect(selectAll).not.toBeChecked();

    const btn = commitBtn();
    expect(btn).toBeDisabled();
    fireEvent.click(btn);
    expect(commit).not.toHaveBeenCalled();
    expect(commitPaths).not.toHaveBeenCalled();
  });
});
