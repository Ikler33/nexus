import { fireEvent, render, screen, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { HomeView } from './HomeView';
import { useAiFeaturesStore } from '../../stores/aiFeatures';
import { useChatStore } from '../../stores/chat';
import { useHomeStore } from '../../stores/home';
import { useUIStore } from '../../stores/ui';

// Реальный load() стора — восстанавливаем его каждый тест, т.к. отдельные кейсы ниже подменяют его
// no-op'ом (чтобы детерминированно проверить error/loading), иначе подмена утекла бы в соседние тесты.
const realLoad = useHomeStore.getState().load;

function resetStores() {
  useUIStore.setState({ homeOpen: true, newsOpen: false, chatOpen: false });
  useChatStore.setState({ draft: '', pinned: [], mode: 'general', streaming: false });
  // «Инсайты» включены — проверяем рендер карточек вопросов/дрейфа с контентом (отдельный кейс ниже
  // проверяет disabled-состояние при OFF).
  useAiFeaturesStore.setState({ insights: true, contradictions: true });
  useHomeStore.setState({
    data: null,
    activity: null,
    brief: null,
    questions: [],
    drift: null,
    stale: [],
    graph: null,
    loading: true,
    generating: {},
    error: null,
    load: realLoad,
  });
}

describe('HomeView (DP-1, макет home.jsx)', () => {
  beforeEach(resetStores);

  // Дашборд: приветствие, сводка дня (AI-карта из кэша виджета), недавние, статистика,
  // stale radar и открытые вопросы — всё из мок-бэкенда H1/H6/H2.
  it('рендерит секции лендинга из данных бэкенда', async () => {
    render(<HomeView />);

    expect(
      await screen.findByText(/архитектурой агентов/),
    ).toBeInTheDocument(); // сводка дня (bold-фрагмент внутри strong)
    expect(screen.getByText(/добр|good/i)).toBeInTheDocument(); // и «Доброй ночи» (тест в 23–06 ч)
    // «RAG Pipeline» встречается в continue-карте и в недавних.
    expect(screen.getAllByText('RAG Pipeline').length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText(/сводка дня|daily brief/i).length).toBeGreaterThan(0);
    expect(screen.getByText(/недавние|recent/i)).toBeInTheDocument();
    expect(screen.getByText(/статистика|stats/i)).toBeInTheDocument();
    expect(screen.getByText('Roadmap Q1')).toBeInTheDocument(); // stale radar
    expect(screen.getByText(/чанк-перекрытие/)).toBeInTheDocument(); // открытый вопрос
    expect(screen.getByText(/смещение фокуса|focus drift/i)).toBeInTheDocument();
    // Heatmap-сетка построена (17 недель × 7).
    expect(document.querySelectorAll('[class*="heatCell"]').length).toBeGreaterThan(119);
  });

  // owner-тоггл «Инсайты» OFF: AI-карточки (вопросы/дрейф) показывают честную подсказку «включите в
  // настройках» вместо контента; daily_brief гейтится не «Инсайтами» — его сводка остаётся.
  it('«Инсайты» OFF → карточки вопросов/дрейфа в disabled-состоянии, сводка дня остаётся', async () => {
    useAiFeaturesStore.setState({ insights: false });
    render(<HomeView />);
    // Сводка дня (daily_brief) рендерится независимо от тоггла «Инсайты».
    expect(await screen.findByText(/архитектурой агентов/)).toBeInTheDocument();
    // Карточки вопросов и дрейфа → подсказка «выключены» (две: open_questions + context_drift).
    expect(screen.getAllByText(/«Инсайты» выключены|Insights are off/i).length).toBe(2);
    // Контент инсайтов скрыт.
    expect(screen.queryByText(/чанк-перекрытие/)).not.toBeInTheDocument();
  });

  // Клик по недавней заметке открывает её в редакторе и закрывает Home.
  it('недавняя заметка → открытие файла, Home закрывается', async () => {
    render(<HomeView />);
    const row = await screen.findByRole('button', { name: /Embeddings/ });
    fireEvent.click(row);
    await vi.waitFor(() => expect(useUIStore.getState().homeOpen).toBe(false));
  });

  // AIP-6: «Разобрать с ИИ» на открытом вопросе → открыть чат(vault) с prefill вопроса + пином заметки.
  it('«Разобрать с ИИ» на вопросе → чат(vault) с prefill + пин заметки', async () => {
    render(<HomeView />);
    const oq = await screen.findByText(/чанк-перекрытие/); // дождались загрузки открытых вопросов
    // Кнопка discuss именно этого вопроса (у stale-заметок теперь тоже есть discuss — AIP-хвост).
    const row = oq.closest('[class*="oqRow"]') as HTMLElement;
    fireEvent.click(within(row).getByRole('button', { name: /Разобрать с ИИ|Discuss with AI/ }));
    const chat = useChatStore.getState();
    expect(chat.draft).toMatch(/чанк-перекрытие/); // композер предзаполнен текстом вопроса
    expect(chat.mode).toBe('vault'); // режим «по заметкам»
    expect(chat.pinned).toContain('Research/RAG Pipeline.md'); // заметка-источник закреплена
    expect(useUIStore.getState().chatOpen).toBe(true); // чат открыт
  });

  // AIP-хвост: «Разобрать с ИИ» на stale-заметке → чат(vault) с prefill-промптом + пином заметки.
  it('«Разобрать с ИИ» на stale-заметке → чат(vault) + пин устаревшей заметки', async () => {
    render(<HomeView />);
    const note = await screen.findByText('Roadmap Q1'); // первая stale-заметка мока
    const row = note.closest('[class*="staleRow"]') as HTMLElement;
    fireEvent.click(within(row).getByRole('button', { name: /Разобрать с ИИ|Discuss with AI/ }));
    const chat = useChatStore.getState();
    expect(chat.mode).toBe('vault');
    expect(chat.draft).toMatch(/Roadmap Q1/); // промпт упоминает заметку
    expect(chat.pinned).toContain('Projects/Roadmap Q1.md'); // устаревшая заметка закреплена
    expect(useUIStore.getState().chatOpen).toBe(true);
  });

  // AIP-6: CTA «Разобрать с ИИ» под дрейфом → prefill с шаблоном расхождения (без пина).
  it('«Разобрать с ИИ» на дрейфе → prefill шаблоном (без пина)', async () => {
    render(<HomeView />);
    await screen.findByText(/смещение фокуса|focus drift/i);
    const discuss = screen.getAllByRole('button', { name: /Разобрать с ИИ|Discuss with AI/ });
    fireEvent.click(discuss[discuss.length - 1]); // последний — CTA дрейфа (после вопросов)
    expect(useChatStore.getState().draft).toMatch(/вернуть фокус/); // шаблон drift
    expect(useChatStore.getState().pinned).toHaveLength(0); // у дрейфа нет одной заметки-источника
  });

  // audit B13: error/loading из стора теперь видимы (раньше деструктуризация их игнорировала).
  it('показывает error-баннер при ошибке загрузки', () => {
    useHomeStore.setState({ error: 'network down', loading: false, data: null, load: async () => {} });
    render(<HomeView />);
    expect(screen.getByRole('alert')).toHaveTextContent(/не удалось загрузить|failed to load/i);
  });

  it('показывает loading-хинт при первой загрузке (data ещё нет)', () => {
    useHomeStore.setState({ loading: true, data: null, error: null, load: async () => {} });
    render(<HomeView />);
    expect(screen.getByText(/^загрузка…$|^loading…$/i)).toBeInTheDocument();
  });

  // «Обновить» на AI-карте ставит фоновую генерацию: thinking-оверлей до прихода результата.
  it('refresh AI-виджета показывает thinking и возвращает контент (мок)', async () => {
    render(<HomeView />);
    await screen.findByText(/архитектурой агентов/);
    const refreshButtons = screen.getAllByRole('button', { name: /обновить|refresh/i });
    fireEvent.click(refreshButtons[0]); // сводка дня
    expect(await screen.findByText(/анализирую|analyzing/i)).toBeInTheDocument();
    await vi.waitFor(
      () => expect(screen.queryByText(/анализирую|analyzing/i)).not.toBeInTheDocument(),
      { timeout: 3000 },
    );
  });
});
