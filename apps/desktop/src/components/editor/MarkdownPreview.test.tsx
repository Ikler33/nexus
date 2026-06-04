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
});
