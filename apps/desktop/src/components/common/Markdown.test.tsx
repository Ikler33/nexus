import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { Markdown } from './Markdown';

// mermaid рендерится через тяжёлый dynamic-import + getBBox (jsdom не умеет) — мокаем renderMermaid
// фейковым CSP-безопасным SVG, проверяем только конвейер фенс→компонент (W-35).
vi.mock('../../lib/markdown/mermaid', () => ({
  renderMermaid: vi.fn(
    async () => '<svg xmlns="http://www.w3.org/2000/svg" data-mmd="1"><rect width="10" height="10"/></svg>',
  ),
}));

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

describe('Markdown — mermaid (W-35: по умолчанию вкл)', () => {
  it('фенс ```mermaid → MermaidDiagram (SVG), а не обычный код-блок', async () => {
    const { container } = render(<Markdown content={'```mermaid\ngraph TD; A-->B;\n```'} />);
    await waitFor(() => expect(container.querySelector('svg[data-mmd]')).not.toBeNull());
    expect(container.querySelector('code')).toBeNull(); // фенс ушёл в диаграмму, не код-блок
  });

  it('обычный ```js фенс остаётся код-блоком (mermaid не трогает чужие языки)', () => {
    const { container } = render(<Markdown content={'```js\nconst x = 1;\n```'} />);
    expect(container.querySelector('code')).not.toBeNull();
    expect(container.querySelector('svg[data-mmd]')).toBeNull();
  });

  it('mermaid={false}: фенс ```mermaid рисуется обычным код-блоком (для стрима)', () => {
    const { container } = render(
      <Markdown content={'```mermaid\ngraph TD; A-->B;\n```'} mermaid={false} />,
    );
    expect(container.querySelector('svg[data-mmd]')).toBeNull();
    expect(container.querySelector('code')).not.toBeNull();
  });
});
