import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { NewsDiagnostics } from './NewsDiagnostics';
import { tauriApi, type NewsRun } from '../../lib/tauri-api';

function run(over: Partial<NewsRun> = {}): NewsRun {
  return {
    runAt: 1_700_000_000,
    digestRu: 'дайджест',
    itemsNew: 3,
    sourcesOk: 6,
    sourcesTotal: 6,
    llmFailed: 0,
    errors: [],
    ...over,
  };
}

/** Открывает панель «Диагностика» (кнопка-тогглер у шапки ленты). */
function openPanel() {
  fireEvent.click(screen.getByRole('button', { name: /диагностика загрузки|loading diagnostics/i }));
}

describe('NewsDiagnostics (W-39)', () => {
  // Панель открывается и лениво рендерит историю прогонов из мока (компактный список).
  it('история: открытие панели рендерит прогоны из мока', async () => {
    render(<NewsDiagnostics lastRun={run()} feedEmpty={false} />);
    openPanel();

    // Заголовок панели + блок последнего прогона.
    expect(screen.getByText(/^диагностика$|^diagnostics$/i)).toBeInTheDocument();
    expect(screen.getByText(/последний прогон|last run/i)).toBeInTheDocument();

    // История подтягивается из мока (несколько прогонов, свежие сверху) — видны метки +N.
    expect(await screen.findByText(/история прогонов|run history/i)).toBeInTheDocument();
    expect((await screen.findAllByText(/^\+\d+$/)).length).toBeGreaterThanOrEqual(2);
  });

  // «Проверить связь» зовёт testEndpoint и показывает пилюлю статуса с endpoint + латентностью.
  it('проверить связь: зовёт testEndpoint и показывает ✓ + endpoint + латентность', async () => {
    const spy = vi.spyOn(tauriApi.news, 'testEndpoint');
    render(<NewsDiagnostics lastRun={run()} feedEmpty={false} />);
    openPanel();

    fireEvent.click(screen.getByRole('button', { name: /проверить связь|test connection/i }));
    expect(spy).toHaveBeenCalledTimes(1);

    // Пилюля статуса (latest-wins): «Связь есть» + endpoint + латентность.
    expect(await screen.findByText(/связь есть|connected/i)).toBeInTheDocument();
    expect(screen.getByText(/localhost:8084/)).toBeInTheDocument();
    expect(screen.getByText(/\d+\s*(мс|ms)/i)).toBeInTheDocument();
    spy.mockRestore();
  });

  // «Проверить связь» при недоступном эндпоинте → пилюля ✗ с сообщением причины.
  it('проверить связь: провал → ✗ с причиной', async () => {
    const spy = vi.spyOn(tauriApi.news, 'testEndpoint').mockResolvedValue({
      ok: false,
      message: 'эндпоинт недоступен: connection refused',
      endpoint: 'http://192.168.0.28:8084',
      latencyMs: 12,
    });
    render(<NewsDiagnostics lastRun={run()} feedEmpty={false} />);
    openPanel();

    fireEvent.click(screen.getByRole('button', { name: /проверить связь|test connection/i }));
    expect(await screen.findByText(/нет связи|no connection/i)).toBeInTheDocument();
    expect(screen.getByText(/connection refused/i)).toBeInTheDocument();
    spy.mockRestore();
  });

  // Empty-state по ПРИЧИНЕ (legacy-путь): старая запись news_runs несёт ТОЛЬКО RU-строку в errors[]
  // (без поля llmDown) → deprecated-сниффер всё ещё распознаёт её и называет эндпоинт.
  it('пусто: причина — анализатор недоступен (legacy-строка без llmDown)', async () => {
    const lastRun = run({
      itemsNew: 0,
      llmFailed: 5,
      errors: [
        'Анализатор новостей недоступен: http://192.168.0.28:8084 — 5 зап. не оценены; лента не обновится, пока эндпоинт не починить (Настройки → ИИ)',
      ],
    });
    render(<NewsDiagnostics lastRun={lastRun} feedEmpty={true} />);
    openPanel();

    expect(await screen.findByText(/почему пусто|why empty/i)).toBeInTheDocument();
    // Эндпоинт назван в блоке причины (и заодно в списке ошибок последнего прогона) — оба места.
    expect(screen.getAllByText(/localhost:8084/).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText(/анализатор недоступен|analyzer unavailable/i)).toBeInTheDocument();
  });

  // B12: структурное поле llmDown → причина с эндпоинтом БЕЗ строки-протокола в errors[] (regex
  // не задействован — эндпоинт берётся из поля).
  it('пусто: причина — анализатор недоступен (структурное поле llmDown)', async () => {
    const lastRun = run({
      itemsNew: 0,
      llmFailed: 5,
      errors: [],
      llmDown: { endpoint: 'http://10.0.0.7:8084', partial: false },
    });
    render(<NewsDiagnostics lastRun={lastRun} feedEmpty={true} />);
    openPanel();

    expect(await screen.findByText(/почему пусто|why empty/i)).toBeInTheDocument();
    expect(screen.getByText(/10\.0\.0\.7:8084/)).toBeInTheDocument();
    expect(screen.getByText(/анализатор недоступен|analyzer unavailable/i)).toBeInTheDocument();
  });

  // B12: llmDown без эндпоинта (не задан) → generic-формулировка (без пустого «: —»).
  it('пусто: llmDown без эндпоинта → generic-причина', async () => {
    const lastRun = run({
      itemsNew: 0,
      llmFailed: 2,
      errors: [],
      llmDown: { endpoint: null, partial: false },
    });
    render(<NewsDiagnostics lastRun={lastRun} feedEmpty={true} />);
    openPanel();
    expect(
      await screen.findByText(/был недоступен в последнем прогоне|was unavailable in the last run/i),
    ).toBeInTheDocument();
  });

  // Empty-state по ПРИЧИНЕ: источники не ответили (sourcesOk==0).
  it('пусто: причина — источники не ответили', async () => {
    render(
      <NewsDiagnostics lastRun={run({ itemsNew: 0, sourcesOk: 0, sourcesTotal: 6 })} feedEmpty={true} />,
    );
    openPanel();
    expect(
      await screen.findByText(/источники не ответили|sources didn.t respond/i),
    ).toBeInTheDocument();
  });

  // Empty-state по ПРИЧИНЕ: всё дедуплицировано (sources ok, нет новых).
  it('пусто: причина — всё дедуплицировано', async () => {
    render(
      <NewsDiagnostics lastRun={run({ itemsNew: 0, sourcesOk: 6, sourcesTotal: 6 })} feedEmpty={true} />,
    );
    openPanel();
    expect(await screen.findByText(/дедуп|dedup/i)).toBeInTheDocument();
  });

  // Лента НЕ пуста → блока «почему пусто» нет (без ложной тревоги).
  it('лента не пуста: блока «почему пусто» нет', async () => {
    render(<NewsDiagnostics lastRun={run()} feedEmpty={false} />);
    openPanel();
    // Дождёмся подгрузки истории (ленивый промис), затем проверим отсутствие блока причины.
    expect(await screen.findByText(/история прогонов|run history/i)).toBeInTheDocument();
    expect(screen.queryByText(/почему пусто|why empty/i)).toBeNull();
  });
});
