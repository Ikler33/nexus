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

  it('related/summary — структура + заглушка, БЕЗ LLM-вызова', () => {
    const relatedSpy = vi.spyOn(tauriApi.suggest, 'related');
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Похожие' }));
    expect(screen.getByText(/Нужен AI/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Резюме' }));
    expect(screen.getByText(/Нужен AI/)).toBeInTheDocument();
    expect(relatedSpy).not.toHaveBeenCalled(); // НИ одного LLM/suggest-вызова на этом срезе
  });

  it('повторный клик по активному тогглу сворачивает панель', () => {
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    const toggle = screen.getByRole('button', { name: 'Резюме' });
    fireEvent.click(toggle);
    expect(screen.getByText(/Нужен AI/)).toBeInTheDocument();
    fireEvent.click(toggle); // повторно → закрыть
    expect(screen.queryByText(/Нужен AI/)).toBeNull();
  });

  it('кнопка-крестик сворачивает открытую панель', () => {
    render(<InspectorRail doc={DOC} path="A.md" onJump={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Похожие' }));
    fireEvent.click(screen.getByRole('button', { name: 'Свернуть панель' }));
    expect(screen.queryByText(/Нужен AI/)).toBeNull();
  });
});
