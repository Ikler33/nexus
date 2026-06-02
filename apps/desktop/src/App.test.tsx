import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from './App';

describe('App (caркас Ф0-1)', () => {
  it('рендерит заголовок Nexus', () => {
    render(<App />);
    expect(screen.getByRole('heading', { name: 'Nexus' })).toBeInTheDocument();
  });

  it('в браузерном окружении показывает версию-заглушку', () => {
    render(<App />);
    // isTauri() === false в jsdom → IPC не вызывается, остаётся 'dev'.
    expect(screen.getByText('vdev')).toBeInTheDocument();
  });
});
