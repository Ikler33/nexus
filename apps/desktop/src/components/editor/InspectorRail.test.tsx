import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { tauriApi } from '../../lib/tauri-api';
import { InspectorRail } from './InspectorRail';

afterEach(() => vi.restoreAllMocks());

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

  it('кнопка-крестик сворачивает открытую панель', () => {
    vi.spyOn(tauriApi.suggest, 'related').mockResolvedValue([]);
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Похожие' }));
    expect(screen.getByRole('button', { name: 'Свернуть панель' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Свернуть панель' }));
    expect(screen.queryByRole('button', { name: 'Свернуть панель' })).toBeNull();
  });
});
