import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { tauriApi } from '../../lib/tauri-api';
import { useUIStore } from '../../stores/ui';
import { InspectorRail } from './InspectorRail';

afterEach(() => {
  vi.restoreAllMocks();
  useUIStore.setState({ pendingInspectorSection: null });
});

const DOC = ['# Intro', '## Details', 'body'].join('\n');

describe('InspectorRail (editor-chrome)', () => {
  it('по умолчанию панель свёрнута — видны только тогглы rail', () => {
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    // 4 тоггла rail
    expect(screen.getByRole('button', { name: 'Оглавление' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Связи' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Похожие' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Резюме' })).toBeInTheDocument();
    // панель не открыта → нет заголовка оглавления-секции из OutlineBar
    expect(screen.queryByRole('button', { name: 'Intro' })).toBeNull();
  });

  it('тоггл «Оглавление» открывает панель с существующим OutlineBar; клик зовёт onJump', () => {
    const onJump = vi.fn();
    render(<InspectorRail doc={DOC} path="A.md" onJump={onJump} />);
    fireEvent.click(screen.getByRole('button', { name: 'Оглавление' }));
    // OutlineBar отрисовал заголовки заметки
    fireEvent.click(screen.getByRole('button', { name: 'Details' }));
    expect(onJump).toHaveBeenCalledWith(2); // `## Details` — строка 2
  });

  // Hermes-8 S6 scroll-spy: activeLine прокидывается в OutlineBar → подсветка активного пункта.
  it('S6: activeLine доходит до OutlineBar (активный пункт несёт aria-current=location)', () => {
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} activeLine={2} />);
    fireEvent.click(screen.getByRole('button', { name: 'Оглавление' }));
    expect(screen.getByRole('button', { name: 'Details' })).toHaveAttribute('aria-current', 'location');
  });

  it('тоггл «Связи» открывает панель с существующим BacklinksBar', async () => {
    vi.spyOn(tauriApi.graph, 'getBacklinks').mockResolvedValue([
      { sourcePath: 'Notes/Linker.md', sourceTitle: 'Linker', context: null, lineNumber: null },
    ]);
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Связи' }));
    expect(await screen.findByText('Linker')).toBeInTheDocument(); // BacklinksBar отработал
  });

  it('«Похожие» грузит suggest.related, «Резюме» — suggest.noteSummary', async () => {
    const relatedSpy = vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([]);
    const summarySpy = vi.spyOn(tauriApi.suggest, 'noteSummary').mockResolvedValue(null);
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Похожие' }));
    expect(await screen.findByText('Нет похожих заметок')).toBeInTheDocument();
    expect(relatedSpy).toHaveBeenCalledWith('A.md', expect.any(Number));
    fireEvent.click(screen.getByRole('button', { name: 'Резюме' }));
    expect(await screen.findByText(/Нет резюме/)).toBeInTheDocument();
    expect(summarySpy).toHaveBeenCalledWith(DOC);
  });

  it('повторный клик по активному тогглу сворачивает панель', () => {
    vi.spyOn(tauriApi.suggest, 'noteSummary').mockResolvedValue(null);
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    const toggle = screen.getByRole('button', { name: 'Резюме' });
    fireEvent.click(toggle);
    expect(screen.getByRole('button', { name: 'Свернуть панель' })).toBeInTheDocument();
    fireEvent.click(toggle); // повторно → закрыть (компонент размонтируется до резолва промиса)
    expect(screen.queryByRole('button', { name: 'Свернуть панель' })).toBeNull();
  });

  // Релокация «Связи» (Hermes-6): команда палитры view.suggest ставит pendingInspectorSection;
  // InspectorRail открывает секцию и сбрасывает запрос (паттерн pendingTagFilter).
  it('открывает секцию по pendingInspectorSection и сбрасывает запрос', async () => {
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([]);
    useUIStore.setState({ pendingInspectorSection: 'suggest' });
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    // Панель раскрылась на запрошенной секции (виден крестик «Свернуть панель»).
    expect(await screen.findByRole('button', { name: 'Свернуть панель' })).toBeInTheDocument();
    // Отложенный запрос потреблён (не залипает на следующий маунт).
    expect(useUIStore.getState().pendingInspectorSection).toBeNull();
  });

  it('кнопка-крестик сворачивает открытую панель', () => {
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([]);
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Похожие' }));
    expect(screen.getByRole('button', { name: 'Свернуть панель' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Свернуть панель' }));
    expect(screen.queryByRole('button', { name: 'Свернуть панель' })).toBeNull();
  });

  // S6b рескин: активный тоггл несёт ember-маркер (класс `on`) + aria-pressed; неактивные — нет.
  it('активный тоггл получает ember-класс «on» и aria-pressed', () => {
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([]);
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    const related = screen.getByRole('button', { name: 'Похожие' });
    const outline = screen.getByRole('button', { name: 'Оглавление' });
    // CSS-модуль хэширует имя класса в `_on_<hash>` — матчим по префиксу `_on_`.
    const ON = /(^|\s)_on_/;
    // до активации — без маркера
    expect(related.className).not.toMatch(ON);
    expect(related).toHaveAttribute('aria-pressed', 'false');
    fireEvent.click(related);
    expect(related.className).toMatch(ON); // ember-подсветка активной секции
    expect(related).toHaveAttribute('aria-pressed', 'true');
    expect(outline.className).not.toMatch(ON); // другие — неактивны
  });
});
