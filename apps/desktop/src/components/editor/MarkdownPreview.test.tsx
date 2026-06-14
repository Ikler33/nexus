import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { MarkdownPreview } from './MarkdownPreview';

afterEach(() => vi.restoreAllMocks());

describe('MarkdownPreview (#20)', () => {
  it('рендерит markdown: заголовок, жирный, список', () => {
    render(<MarkdownPreview source={'# Title\n\n**bold** text\n\n- one\n- two'} onOpenLink={() => {}} />);
    expect(screen.getByRole('heading', { name: 'Title' })).toBeInTheDocument();
    expect(screen.getByText('bold')).toBeInTheDocument();
    expect(screen.getByText('one')).toBeInTheDocument();
    expect(screen.getAllByRole('listitem')).toHaveLength(2);
  });

  it('GFM таблицы/таск-листы', () => {
    const src = '| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo';
    render(<MarkdownPreview source={src} onOpenLink={() => {}} />);
    expect(screen.getByRole('table')).toBeInTheDocument();
    expect(screen.getAllByRole('checkbox')).toHaveLength(2);
  });

  it('[[wikilink]] кликается → onOpenLink(target)', () => {
    const onOpen = vi.fn();
    render(<MarkdownPreview source={'go to [[My Note]] now'} onOpenLink={onOpen} />);
    const link = screen.getByText('My Note');
    fireEvent.click(link);
    expect(onOpen).toHaveBeenCalledWith('My Note');
  });

  it('#tag рендерится как чип (не ссылка)', () => {
    render(<MarkdownPreview source={'topic #ideas end'} onOpenLink={() => {}} />);
    expect(screen.getByText('#ideas')).toBeInTheDocument();
  });

  it('внутри code-fence [[x]] НЕ превращается в ссылку (mdast-уровень)', () => {
    render(<MarkdownPreview source={'```\n[[NotALink]]\n```'} onOpenLink={() => {}} />);
    // Литерал с квадратными скобками сохранён → трансформации не было.
    expect(screen.getByText(/\[\[NotALink\]\]/)).toBeInTheDocument();
  });

  it('javascript:-URL вырезается (анти-XSS, CSP-safe)', () => {
    render(<MarkdownPreview source={'[click](javascript:alert(1))'} onOpenLink={() => {}} />);
    const link = screen.getByText('click');
    expect(link.getAttribute('href') ?? '').not.toMatch(/javascript:/i);
  });

  it('EDIT-5: с onToggleTask чекбоксы кликабельны → 1-based номер исходной строки', () => {
    const onToggle = vi.fn();
    render(
      <MarkdownPreview source={'- [x] done\n- [ ] todo'} onOpenLink={() => {}} onToggleTask={onToggle} />,
    );
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(2);
    expect(boxes[0]).not.toBeDisabled(); // интерактивный, не дефолтный disabled
    fireEvent.click(boxes[1]); // второй таск — на исходной строке 2
    expect(onToggle).toHaveBeenCalledWith(2);
  });

  it('EDIT-5: без onToggleTask чекбоксы остаются read-only (disabled)', () => {
    render(<MarkdownPreview source={'- [ ] todo'} onOpenLink={() => {}} />);
    expect(screen.getByRole('checkbox')).toBeDisabled();
  });

  // Регресс на находку ревью: таск в цитате — исходная строка `> - [x]` не тогглится, значит
  // честный read-only с ВЕРНЫМ состоянием (из GFM), а не «кликабельный» чекбокс-пустышка.
  it('EDIT-5: таск в цитате — read-only с верным состоянием, клик не вызывает onToggleTask', () => {
    const onToggle = vi.fn();
    render(<MarkdownPreview source={'> - [x] quoted'} onOpenLink={() => {}} onToggleTask={onToggle} />);
    const box = screen.getByRole('checkbox');
    expect(box).toBeChecked();
    expect(box).toBeDisabled();
  });

  // Регресс на находку ревью: в loose-списке GFM кладёт input внутрь <p> — он не должен задвоиться
  // с нашим (input=>null убирает все дефолтные, в т.ч. вложенные).
  it('EDIT-5: loose-список — ровно один интерактивный чекбокс на пункт', () => {
    const onToggle = vi.fn();
    render(<MarkdownPreview source={'- [ ] a\n\n- [ ] b'} onOpenLink={() => {}} onToggleTask={onToggle} />);
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(2);
    boxes.forEach((b) => expect(b).not.toBeDisabled());
  });

  // Регресс на находку реверификации: состояние родителя не должно подменяться отмеченным дочерним
  // таском (ownTaskChecked не спускается в подсписок).
  it('EDIT-5: вложенный подсписок — состояние родителя не подменяется дочерним', () => {
    const onToggle = vi.fn();
    render(
      <MarkdownPreview source={'- [ ] parent\n  - [x] child'} onOpenLink={() => {}} onToggleTask={onToggle} />,
    );
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(2);
    expect(boxes[0]).not.toBeChecked(); // родитель НЕ отмечен
    expect(boxes[1]).toBeChecked(); // ребёнок отмечен
  });

  it('IMG-1: vault-картинка грузится как data:-URL через read_attachment', async () => {
    const read = vi
      .spyOn(tauriApi.attachments, 'read')
      .mockResolvedValue('data:image/png;base64,AAAA');
    render(<MarkdownPreview source={'![кот](attachments/cat.png)'} onOpenLink={() => {}} />);
    const img = await screen.findByRole('img', { name: 'кот' });
    expect(read).toHaveBeenCalledWith('attachments/cat.png');
    await waitFor(() => expect(img).toHaveAttribute('src', 'data:image/png;base64,AAAA'));
  });

  it('IMG-1: внешний URL картинки НЕ идёт через read_attachment', async () => {
    const read = vi.spyOn(tauriApi.attachments, 'read');
    render(<MarkdownPreview source={'![ext](https://example.com/x.png)'} onOpenLink={() => {}} />);
    const img = await screen.findByRole('img', { name: 'ext' });
    expect(img).toHaveAttribute('src', 'https://example.com/x.png');
    expect(read).not.toHaveBeenCalled();
  });
});

describe('MarkdownPreview: формулы (#4, MathML под строгим CSP)', () => {
  it('инлайн $$…$$ рендерится в <math>', () => {
    const { container } = render(<MarkdownPreview source={'энергия $$E=mc^2$$ тут'} onOpenLink={() => {}} />);
    const math = container.querySelector('math');
    expect(math).not.toBeNull();
    expect(math?.querySelector('msup')).not.toBeNull(); // mc^2 → надстрочный
  });

  it('блочная $$…$$ рендерится в <math>', () => {
    const { container } = render(
      <MarkdownPreview source={'$$\\int_0^1 x\\,dx$$'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).not.toBeNull();
  });

  // Регресс: $$ внутри инлайн-кода НЕ парсится как математика (remark-math уважает приоритет code).
  it('$$ внутри инлайн-кода остаётся литералом, не математикой', () => {
    const { container } = render(<MarkdownPreview source={'код `$$x$$` тут'} onOpenLink={() => {}} />);
    expect(container.querySelector('math')).toBeNull();
    expect(screen.getByText('$$x$$')).toBeInTheDocument();
  });

  it('$$ внутри code-fence не превращается в формулу', () => {
    const { container } = render(
      <MarkdownPreview source={'```\n$$a+b$$\n```'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).toBeNull();
  });

  // Сосуществование: math + wikilink + tag на одной строке — remarkNexus не сломан remark-math.
  it('формула уживается с [[wikilink]] и #tag', () => {
    const { container } = render(
      <MarkdownPreview source={'$$x$$ и [[Заметка]] и #идея'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).not.toBeNull();
    expect(screen.getByText('Заметка')).toBeInTheDocument();
    expect(screen.getByText('#идея')).toBeInTheDocument();
  });

  // БАГ adversarial-ревью реализации #5: двойной-$ валюты в заметках о деньгах НЕ должен становиться
  // математикой (политика singleDollarTextMath:false — одиночный $ = валюта, не формула).
  it('суммы с $ (валюта) не парсятся как формула', () => {
    const { container } = render(
      <MarkdownPreview source={'зарплата с $5000 до $7000 и ещё $200'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).toBeNull();
    expect(screen.getByText(/5000/)).toBeInTheDocument();
  });

  // Блокер adversarial-ревью дизайна: фиксируем output:'mathml' — на валидной формуле НЕТ инлайн-стилей
  // (регресс к default htmlAndMathml вернул бы сотни style="" на span → CSP-violation, тихо в jsdom).
  it('CSP-guard: валидная формула не содержит ни одного инлайн-style', () => {
    const { container } = render(<MarkdownPreview source={'$$a^2+b^2=c^2$$'} onOpenLink={() => {}} />);
    expect(container.querySelector('math')).not.toBeNull();
    expect(container.querySelectorAll('[style]')).toHaveLength(0);
  });

  // Блокер adversarial-ревью дизайна (error-path): битый LaTeX не роняет рендер И не оставляет инлайн-
  // style на .katex-error (rehypeKatexCsp снял его; цвет — через CSS-класс). Иначе CSP-violation в вебвью.
  it('битый LaTeX: фолбэк без краша и без инлайн-style (CSP-safe)', () => {
    const { container } = render(<MarkdownPreview source={'сломано $$\\frac{1}{$$'} onOpenLink={() => {}} />);
    expect(container.querySelectorAll('[style]')).toHaveLength(0);
  });

  // БАГ adversarial-ревью реализации #2: \fcolorbox даёт <mpadded style="border:…"> ДАЖЕ под output:
  // 'mathml' — стриппер обязан снять и его (не только .katex-error), иначе CSP-violation на валидной формуле.
  it('\\fcolorbox: рамочный стиль снят (CSP-safe, не только error-узлы)', () => {
    const { container } = render(
      <MarkdownPreview source={'$$\\fcolorbox{red}{yellow}{x}$$'} onOpenLink={() => {}} />,
    );
    expect(container.querySelector('math')).not.toBeNull();
    expect(container.querySelectorAll('[style]')).toHaveLength(0);
  });
});
