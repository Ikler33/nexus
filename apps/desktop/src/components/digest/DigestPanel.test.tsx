import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { DigestPanel } from './DigestPanel';
import { useDigestStore } from '../../stores/digest';
import { useUIStore } from '../../stores/ui';

afterEach(() => {
  vi.restoreAllMocks();
  useUIStore.setState({ digestOpen: false });
  useDigestStore.setState({ latest: null, loading: false, generating: false, error: null, baseline: null });
});

describe('DigestPanel (#35, ADR-007 slice 4)', () => {
  it('рендерит последний дайджест (контент + мета с числом заметок)', async () => {
    useUIStore.setState({ digestOpen: true });
    render(<DigestPanel />);

    // Мок отдаёт пример дайджеста (3 заметки).
    expect(await screen.findByText(/введение в книге/i)).toBeInTheDocument();
    expect(screen.getByText(/заметок: 3|notes: 3/i)).toBeInTheDocument();
  });

  it('кнопка «Сгенерировать» ставит генерацию и показывает прогресс-состояние', async () => {
    useUIStore.setState({ digestOpen: true });
    render(<DigestPanel />);
    await screen.findByText(/введение в книге/i); // дождались первичной загрузки

    fireEvent.click(screen.getByTitle(/Сгенерировать|Generate/i));
    expect(useDigestStore.getState().generating).toBe(true);
    expect(await screen.findByText(/Генерирую…|Generating…/i)).toBeInTheDocument();
  });
});
