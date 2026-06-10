import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { NewsView } from './NewsView';
import { tauriApi } from '../../lib/tauri-api';
import { useNewsStore } from '../../stores/news';

/** Сброс стора между тестами (мок-бэкенд news стейтфул на сессию — тесты возвращают всё как было). */
function resetStore() {
  useNewsStore.setState({
    items: [],
    topics: [],
    run: null,
    config: null,
    sources: [],
    topic: null,
    unreadOnly: false,
    loading: true,
    refreshing: false,
    error: null,
    notice: null,
  });
}

describe('NewsView (NF-5, спека docs/specs/news-feed.md)', () => {
  beforeEach(resetStore);

  // AC-NF-9/10: страница рендерит сводку дня, рубрики-кластеры, карточки с метаданными;
  // LLM-фейл записи — «резюме недоступно», оригинальный заголовок остаётся видимым.
  it('лента: сводка дня + рубрики + карточки; пустое резюме → «недоступно»', async () => {
    render(<NewsView />);

    // Сводка дня (AI-карточка) из последнего прогона.
    expect(await screen.findByText(/сводка дня|daily digest/i)).toBeInTheDocument();
    expect(screen.getByText(/главное за сутки/i)).toBeInTheDocument();

    // Рубрики тем в потоке (тема встречается и чипом, и заголовком рубрики) + карточки.
    expect(screen.getAllByText('Модели').length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText('Исследования').length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText(/GPT-5\.2 получил режим длинного контекста/)).toBeInTheDocument();

    // Запись с пустым summary (LLM-фейл) показана с пометкой, заголовок цел (AC-NF-10).
    expect(
      screen.getByText('Новый метод дистилляции снижает галлюцинации на 40%'),
    ).toBeInTheDocument();
    expect(screen.getByText(/резюме недоступно|summary unavailable/i)).toBeInTheDocument();

    // Язык оригинала виден (EN-источники + RU Хабр-кейс в моке).
    expect(screen.getAllByText('EN').length).toBeGreaterThan(0);
  });

  // AC-NF-1 (no silent caps): «K из M источников» раскрывает список ошибок прогона.
  it('частичный прогон: варнинг источников раскрывает ошибки', async () => {
    render(<NewsView />);
    const warn = await screen.findByRole('button', { name: /5 из 6|5 of 6/i });
    fireEvent.click(warn);
    expect(screen.getByText(/таймаут/i)).toBeInTheDocument();
  });

  // AC-NF-9: отметка прочитанного зовёт API и тускнит карточку (оптимистично).
  it('прочитано: тоггл зовёт news.markRead и переключается обратно', async () => {
    const spy = vi.spyOn(tauriApi.news, 'markRead');
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);

    const readBtns = screen.getAllByRole('button', { name: /^прочитано$|^mark as read$/i });
    fireEvent.click(readBtns[0]);
    expect(spy).toHaveBeenCalledWith(expect.any(Number), true);

    // Вернуть как было (мок-состояние общее): теперь кнопка — «вернуть в непрочитанные».
    const unreadBtn = await screen.findAllByRole('button', {
      name: /вернуть в непрочитанные|mark as unread/i,
    });
    fireEvent.click(unreadBtn[0]);
    expect(spy).toHaveBeenLastCalledWith(expect.any(Number), false);
    spy.mockRestore();
  });

  // AC-NF-11: «в заметку» зовёт API и показывает путь созданной заметки.
  it('в заметку: тост с путём News/…', async () => {
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);
    fireEvent.click(screen.getAllByRole('button', { name: /^в заметку$|^to note$/i })[0]);
    expect(await screen.findByRole('status')).toHaveTextContent(/News\//);
  });

  // AC-NF-7 (consent): фича выключена → onboarding-CTA с информированным согласием
  // (число и список доверенных источников); «Включить» пишет конфиг enabled=true.
  it('фича выключена: CTA + согласие с источниками; включение пишет конфиг', async () => {
    const before = await tauriApi.news.getConfig();
    await tauriApi.news.setConfig({ ...before, enabled: false });
    const spy = vi.spyOn(tauriApi.news, 'setConfig');

    render(<NewsView />);
    expect(await screen.findByText(/лента ai-новостей|ai news feed/i)).toBeInTheDocument();
    expect(screen.getByText(/информированное согласие|informed consent/i)).toBeInTheDocument();
    expect(screen.getByText(/OpenAI · DeepMind/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /включить ленту|enable feed/i }));
    await vi.waitFor(() =>
      expect(spy).toHaveBeenCalledWith(expect.objectContaining({ enabled: true })),
    );
    // CTA уступает место ленте.
    expect(await screen.findByText(/сводка дня|daily digest/i)).toBeInTheDocument();
    spy.mockRestore();
  });

  // Чипы тем — серверный фильтр: выбор темы перезапрашивает страницу и оставляет одну рубрику.
  it('фильтр темы: чип сужает ленту до рубрики', async () => {
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);
    fireEvent.click(screen.getByRole('button', { name: 'Исследования' }));
    await vi.waitFor(() =>
      expect(screen.queryByText(/GPT-5\.2 получил режим/)).not.toBeInTheDocument(),
    );
    expect(
      screen.getByText('Новый метод дистилляции снижает галлюцинации на 40%'),
    ).toBeInTheDocument();
  });
});
