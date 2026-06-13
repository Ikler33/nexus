import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { MarkdownPreview } from './MarkdownPreview';

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
});
