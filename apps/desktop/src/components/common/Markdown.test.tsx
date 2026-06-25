import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Markdown } from './Markdown';

describe('Markdown (общий md-рендер: react-markdown + remark-gfm)', () => {
  it('рендерит заголовок/жирный/список/код/ссылку/таблицу как HTML, а не как сырой md', () => {
    const md = [
      '# Title',
      '',
      'Это **bold** текст.',
      '',
      '- item',
      '',
      '```',
      'code block',
      '```',
      '',
      '[link](https://x)',
      '',
      '| A | B |',
      '| - | - |',
      '| 1 | 2 |',
    ].join('\n');
    const { container } = render(<Markdown content={md} />);

    // Заголовок → <h1> (а не литеральный «# Title»).
    const h1 = container.querySelector('h1');
    expect(h1).not.toBeNull();
    expect(h1?.textContent).toBe('Title');

    // Жирный → <strong>.
    expect(container.querySelector('strong')?.textContent).toBe('bold');

    // Список → <li>.
    expect(screen.getByText('item').closest('li')).not.toBeNull();

    // Фенс-код → <pre><code> (GFM).
    const pre = container.querySelector('pre');
    expect(pre).not.toBeNull();
    expect(pre?.querySelector('code')?.textContent).toContain('code block');

    // Ссылка → <a href> (react-markdown рендерит обычную ссылку).
    const a = container.querySelector('a');
    expect(a).not.toBeNull();
    expect(a?.getAttribute('href')).toBe('https://x');
    expect(a?.textContent).toBe('link');

    // GFM-таблица → <table> с ячейками.
    const table = container.querySelector('table');
    expect(table).not.toBeNull();
    expect(table?.querySelectorAll('td').length).toBeGreaterThanOrEqual(2);

    // Сырые markdown-маркеры НЕ остаются литеральным текстом.
    expect(container.textContent).not.toContain('# Title');
    expect(container.textContent).not.toContain('**bold**');
  });
});
