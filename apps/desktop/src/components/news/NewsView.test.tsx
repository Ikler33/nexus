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

  // Фидбэк владельца: при избытке тем (у него было 47) облако чипов застилало экран. Свёрнуто
  // по умолчанию (14 + «Ещё N»), клик раскрывает всё.
  it('облако тем: при избытке свёрнуто, «Ещё N» раскрывает и сворачивает', async () => {
    const topics = Array.from({ length: 20 }, (_, i) => `Тема ${i + 1}`);
    const items = topics.map((tp, i) => ({
      id: i + 1,
      sourceId: 'hn',
      url: `https://e.com/${i}`,
      titleRu: `Заголовок ${i + 1}`,
      summaryRu: 'резюме',
      topic: tp,
      langRu: false,
      publishedAt: 1_700_000_000,
      read: false,
      commentsUrl: null,
    }));
    const run = {
      runAt: 1_700_000_000,
      digestRu: 'дайджест',
      itemsNew: 20,
      sourcesOk: 1,
      sourcesTotal: 1,
      llmFailed: 0,
      errors: [],
    };
    const spy = vi.spyOn(tauriApi.news, 'page').mockResolvedValue({ items, topics, run });

    render(<NewsView />);
    await screen.findByRole('button', { name: 'Тема 1' });

    // Свёрнуто: 14-я тема видна, 20-я — нет; есть кнопка «Ещё 6».
    expect(screen.getByRole('button', { name: 'Тема 14' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Тема 20' })).not.toBeInTheDocument();
    const more = screen.getByRole('button', { name: /Ещё 6|6 more/i });

    // Раскрыть → все 20 видны + «Свернуть»; повторный клик сворачивает.
    fireEvent.click(more);
    expect(screen.getByRole('button', { name: 'Тема 20' })).toBeInTheDocument();
    const less = screen.getByRole('button', { name: /Свернуть|Collapse/i });
    fireEvent.click(less);
    expect(screen.queryByRole('button', { name: 'Тема 20' })).not.toBeInTheDocument();
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

  // NF-6: клик по заголовку открывает reader (статья помечена прочитанной), приходит полный
  // RU-текст с пометкой «перевод AI»; «Сократить» → тезисы; «К ленте» возвращает.
  it('reader: заголовок → полный перевод; «Сократить» → тезисы; «К ленте» → обратно', async () => {
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);

    fireEvent.click(screen.getByText(/GPT-5\.2 получил режим длинного контекста/));
    expect(await screen.findByText(/OpenAI выпустила обновление GPT-5\.2/)).toBeInTheDocument();
    expect(screen.getByText(/перевод ai|ai translation/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /сократить|summarize/i }));
    expect(await screen.findByText(/Окно контекста расширено до 2M токенов/)).toBeInTheDocument();
    expect(screen.getByText(/кратко|tl;dr/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /к ленте|back to feed/i }));
    expect(await screen.findByText(/сводка дня|daily digest/i)).toBeInTheDocument();

    // Открытие пометило прочитанным — вернуть как было (мок-состояние общее).
    const unread = screen.getAllByRole('button', {
      name: /вернуть в непрочитанные|mark as unread/i,
    });
    fireEvent.click(unread[0]);
  });

  // NF-6 fail-closed: статья на хосте вне доверенных источников (HN-кейс) → политика не делает
  // запрос; reader честно говорит об этом и оставляет резюме + ссылку «Оригинал».
  it('reader: хост вне политики → denied-состояние с «Оригиналом»', async () => {
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);

    fireEvent.click(screen.getByText('Новый метод дистилляции снижает галлюцинации на 40%'));
    expect(
      await screen.findByText(/полный текст недоступен|full text unavailable/i),
    ).toBeInTheDocument();
    const original = screen.getByRole('link', { name: /оригинал|original/i });
    expect(original).toHaveAttribute('href', expect.stringContaining('deepmind'));

    // Per-host consent (ревизия NF-6): кнопка называет ИМЕННО хост статьи; клик зовёт allowHost
    // и перезапрашивает статью (в моке снова denied — состояние остаётся честным).
    const allowSpy = vi.spyOn(tauriApi.news, 'allowHost');
    const articleSpy = vi.spyOn(tauriApi.news, 'article');
    const allowBtn = screen.getByRole('button', { name: /разрешить .*deepmind|allow .*deepmind/i });
    fireEvent.click(allowBtn);
    await vi.waitFor(() => expect(allowSpy).toHaveBeenCalledTimes(1));
    expect(String(allowSpy.mock.calls[0][0])).toContain('deepmind');
    await vi.waitFor(() => expect(articleSpy).toHaveBeenCalled());

    fireEvent.click(screen.getByRole('button', { name: /к ленте|back to feed/i }));
    const unread = await screen.findAllByRole('button', {
      name: /вернуть в непрочитанные|mark as unread/i,
    });
    fireEvent.click(unread[unread.length - 1]);
  });

  // NF-6 хвост: у HN-айтема с внешним url (мок: item с commentsUrl) ридер показывает кнопку
  // «Обсуждение на HN» рядом с «Оригинал», ведущую на HN-тред.
  it('reader: при commentsUrl видна кнопка «Обсуждение на HN»', async () => {
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);

    fireEvent.click(screen.getByText('llama.cpp: офлоад KV-cache на CPU без потери скорости'));
    const discussion = await screen.findByRole('link', {
      name: /обсуждение на hn|discussion on hn/i,
    });
    expect(discussion).toHaveAttribute('href', expect.stringContaining('news.ycombinator.com'));
    // «Оригинал» по-прежнему ведёт на сам url (github).
    expect(screen.getByRole('link', { name: /оригинал|original/i })).toHaveAttribute(
      'href',
      expect.stringContaining('github'),
    );
  });

  // P1-15 (honesty/UX): «Сократить» активна ТОЛЬКО когда у статьи есть тело (article `ready`).
  // Без тела (denied/error/loading) сокращать нечего → клик давал пустой summary/ошибку, поэтому
  // блокировка честна. Сначала — denied-кейс (id=6, хост вне политики): кнопка disabled.
  it('reader: «Сократить» disabled, когда статья не ready (denied — тела нет)', async () => {
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);

    fireEvent.click(screen.getByText('Новый метод дистилляции снижает галлюцинации на 40%'));
    await screen.findByText(/полный текст недоступен|full text unavailable/i);

    const summarizeBtn = screen.getByRole('button', { name: /сократить|summarize/i });
    expect(summarizeBtn).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: /к ленте|back to feed/i }));
    const unread = await screen.findAllByRole('button', {
      name: /вернуть в непрочитанные|mark as unread/i,
    });
    fireEvent.click(unread[unread.length - 1]);
  });

  // P1-15: для ready-статьи (тело есть) «Сократить» включена (happy-path не сломан).
  it('reader: «Сократить» enabled, когда статья ready (тело доступно)', async () => {
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);

    fireEvent.click(screen.getByText(/GPT-5\.2 получил режим длинного контекста/));
    await screen.findByText(/OpenAI выпустила обновление GPT-5\.2/);

    expect(screen.getByRole('button', { name: /сократить|summarize/i })).toBeEnabled();

    fireEvent.click(screen.getByRole('button', { name: /к ленте|back to feed/i }));
    const unread = await screen.findAllByRole('button', {
      name: /вернуть в непрочитанные|mark as unread/i,
    });
    fireEvent.click(unread[0]);
  });

  // W-2: мёртвый LLM-эндпоинт — ВИДИМЫЙ баннер (с названным эндпоинтом из errors[]), при этом
  // существующие статьи всё равно показаны (баннер + список, не «или/или»).
  it('LLM недоступен: баннер с эндпоинтом + список статей рендерятся оба', async () => {
    const items = [
      {
        id: 1,
        sourceId: 'openai',
        url: 'https://e.com/1',
        titleRu: 'Старая статья из прошлого прогона',
        summaryRu: 'резюме',
        topic: 'Модели',
        langRu: false,
        publishedAt: 1_700_000_000,
        read: false,
        commentsUrl: null,
      },
    ];
    const run = {
      runAt: 1_700_000_000,
      digestRu: '',
      itemsNew: 0,
      sourcesOk: 1,
      sourcesTotal: 1,
      llmFailed: 7,
      errors: [
        'Анализатор новостей недоступен: http://192.168.0.31:8084 — 7 зап. не оценены; лента не обновится, пока эндпоинт не починить (Настройки → ИИ)',
      ],
    };
    const spy = vi
      .spyOn(tauriApi.news, 'page')
      .mockResolvedValue({ items, topics: ['Модели'], run });

    render(<NewsView />);
    // Баннер виден и НАЗЫВАЕТ эндпоинт (строка из errors[]).
    expect(await screen.findByText(/Анализатор новостей недоступен/)).toBeInTheDocument();
    expect(screen.getByText(/192\.168\.0\.31:8084/)).toBeInTheDocument();
    // При этом статья из прошлого прогона всё равно показана (не пустой экран).
    expect(screen.getByText('Старая статья из прошлого прогона')).toBeInTheDocument();
    spy.mockRestore();
  });

  // W-2 (ревью): пустая лента + анализатор недоступен → состояние ОШИБКИ, а НЕ «свежих новостей нет»
  // (иначе под красным баннером висело бы противоречивое «всё хорошо, заходите завтра»).
  it('LLM недоступен + лента пуста: ошибка-состояние, не «свежих новостей нет»', async () => {
    const run = {
      runAt: 1_700_000_000,
      digestRu: '',
      itemsNew: 0,
      sourcesOk: 1,
      sourcesTotal: 1,
      llmFailed: 5,
      errors: [
        'Анализатор новостей недоступен: http://192.168.0.28:8084 — 5 зап. не оценены; лента не обновится, пока эндпоинт не починить (Настройки → ИИ)',
      ],
    };
    const spy = vi.spyOn(tauriApi.news, 'page').mockResolvedValue({ items: [], topics: [], run });
    render(<NewsView />);
    expect(await screen.findByText(/Анализатор новостей недоступен/)).toBeInTheDocument();
    // НЕ показываем благостное «свежих новостей нет».
    expect(screen.queryByText(/Свежих новостей нет|No fresh news/)).toBeNull();
    // Показываем состояние ошибки сбора ленты (с «Повторить»).
    expect(screen.getByText(/Не удалось собрать ленту|Couldn.t fetch the feed/i)).toBeInTheDocument();
    spy.mockRestore();
  });

  // W-2 регрессия: здоровый прогон (llmFailed=0) → баннера нет (без ложной тревоги).
  it('LLM в норме: баннер недоступности не показывается', async () => {
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);
    expect(screen.queryByText(/Анализатор новостей недоступен|News analyzer unavailable/)).toBeNull();
  });

  // B12: новый бэкенд отдаёт СТРУКТУРНОЕ поле llmDown — баннер строится из него (i18n с эндпоинтом
  // и счётчиком), БЕЗ сниффинга RU-строки в errors[] (в фикстуре строки-протокола нет намеренно).
  it('B12: структурное поле llmDown (тотально) → баннер называет эндпоинт без строки-протокола', async () => {
    const run = {
      runAt: 1_700_000_000,
      digestRu: '',
      itemsNew: 0,
      sourcesOk: 1,
      sourcesTotal: 1,
      llmFailed: 5,
      errors: [],
      llmDown: { endpoint: 'http://10.0.0.7:8084', partial: false },
    };
    const spy = vi.spyOn(tauriApi.news, 'page').mockResolvedValue({ items: [], topics: [], run });
    render(<NewsView />);
    expect(
      await screen.findByText(/Анализатор новостей недоступен|News analyzer unavailable/),
    ).toBeInTheDocument();
    expect(screen.getByText(/10\.0\.0\.7:8084/)).toBeInTheDocument();
    expect(screen.getByText(/5 зап\.|5 entries/)).toBeInTheDocument();
    // Пустая лента при недоступном анализаторе → состояние ошибки, не «свежих новостей нет».
    expect(screen.queryByText(/Свежих новостей нет|No fresh news/)).toBeNull();
    spy.mockRestore();
  });

  // B12: частичный сбой (partial=true, часть батчей прошла) баннер НЕ поднимает — лента обновлена,
  // мягкая строка живёт только в раскрываемом списке ошибок прогона (двухуровневость W-2).
  it('B12: llmDown.partial → баннера нет', async () => {
    const run = {
      runAt: 1_700_000_000,
      digestRu: 'Сводка.',
      itemsNew: 3,
      sourcesOk: 1,
      sourcesTotal: 1,
      llmFailed: 1,
      errors: ['ИИ-анализатор частично недоступен: http://10.0.0.7:8084 — 1 зап. не оценены в этот прогон (повтор при следующем обновлении); остальные новости добавлены'],
      llmDown: { endpoint: 'http://10.0.0.7:8084', partial: true },
    };
    const spy = vi.spyOn(tauriApi.news, 'page').mockResolvedValue({ items: [], topics: [], run });
    render(<NewsView />);
    await screen.findByText(/сводка дня|daily digest/i);
    expect(screen.queryByText(/Анализатор новостей недоступен|News analyzer unavailable/)).toBeNull();
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
