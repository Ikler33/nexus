import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { TaskPeek } from './TaskPeek';
import i18n from '../../i18n/setup';
import { tauriApi, type TaskCard } from '../../lib/tauri-api';

const card: TaskCard = {
  path: 'Tasks/T.md',
  title: 'Дизайн доски',
  status: 'doing',
  project: 'Nexus',
  priority: 'high',
  due: '2026-06-20',
  tags: ['design'],
};

describe('TaskPeek (BOARD-6 — превью задачи, §9)', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('ru');
    vi.restoreAllMocks();
  });

  it('рендерит свойства + ТЕЛО (frontmatter срезан) + «Открыть в редакторе»', async () => {
    vi.spyOn(tauriApi.vault, 'readFileMeta').mockResolvedValue({
      content: '---\nstatus: doing\npriority: high\n---\n# Раздел\nтекст задачи',
      hash: 'h1',
    });
    const onOpenFull = vi.fn();
    render(<TaskPeek card={card} onClose={() => {}} onOpenFull={onOpenFull} onOpenLink={() => {}} />);

    // Свойства из карточки.
    expect(screen.getByText('Дизайн доски')).toBeInTheDocument();
    expect(screen.getByText('doing')).toBeInTheDocument();
    expect(screen.getByText('Nexus')).toBeInTheDocument();
    expect(screen.getByText('#design')).toBeInTheDocument();

    // Тело отрендерилось без frontmatter (заголовок «Раздел» есть, «status: doing» как текст — нет).
    await waitFor(() => expect(screen.getByRole('heading', { name: 'Раздел' })).toBeInTheDocument());
    expect(screen.getByText('текст задачи')).toBeInTheDocument();
    expect(screen.queryByText(/status: doing/)).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /Открыть в редакторе/i }));
    expect(onOpenFull).toHaveBeenCalledWith('Tasks/T.md');
  });

  it('пустое тело → подсказка «нет тела»', async () => {
    vi.spyOn(tauriApi.vault, 'readFileMeta').mockResolvedValue({
      content: '---\nstatus: doing\n---\n',
      hash: 'h2',
    });
    render(<TaskPeek card={card} onClose={() => {}} onOpenFull={() => {}} onOpenLink={() => {}} />);
    await waitFor(() => expect(screen.getByText(/нет тела/i)).toBeInTheDocument());
  });
});
