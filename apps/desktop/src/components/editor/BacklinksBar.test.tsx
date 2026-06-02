import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { BacklinksBar } from './BacklinksBar';

describe('BacklinksBar (Ф0-6/Ф0-9)', () => {
  it('показывает входящие ссылки переданного файла', async () => {
    render(<BacklinksBar path="Inbox.md" />); // на Inbox ссылается README (mock)
    expect(await screen.findByText('README.md')).toBeInTheDocument();
  });

  it('показывает пустое состояние, когда обратных ссылок нет', async () => {
    render(<BacklinksBar path="Notes/Idea.md" />); // на Idea никто не ссылается
    expect(await screen.findByText(/Нет обратных ссылок/)).toBeInTheDocument();
  });
});
