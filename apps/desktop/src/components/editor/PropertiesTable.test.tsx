import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { PropertiesTable } from './PropertiesTable';
import type { FmField } from '../../lib/markdown/frontmatter';

const FIELDS: FmField[] = [
  { key: 'type', values: ['idea'] },
  { key: 'status', values: ['seed'] },
  { key: 'created', values: ['2026-03-06'] },
  { key: 'tags', values: ['мышление', 'психология'] },
];

describe('PropertiesTable (Hermes-8 S4 «Вариант А · Колонка»)', () => {
  it('рендерит ключи и значения frontmatter', () => {
    render(<PropertiesTable fields={FIELDS} />);
    expect(screen.getByText('type')).toBeInTheDocument();
    expect(screen.getByText('idea')).toBeInTheDocument();
    expect(screen.getByText('status')).toBeInTheDocument();
    expect(screen.getByText('seed')).toBeInTheDocument();
    expect(screen.getByText('created')).toBeInTheDocument();
    expect(screen.getByText('2026-03-06')).toBeInTheDocument();
  });

  it('значение ключа `type` получает акцент-класс (.acc → ember)', () => {
    render(<PropertiesTable fields={FIELDS} />);
    const val = screen.getByText('idea');
    // CSS-модуль хэширует имя класса — проверяем по подстроке `acc`.
    expect(val.className).toMatch(/\bacc\b|acc/);
    expect(val.className).toMatch(/propVal/);
  });

  it('акцент не навешивается на не-`type` значения', () => {
    render(<PropertiesTable fields={FIELDS} />);
    const status = screen.getByText('seed');
    expect(status.className).not.toMatch(/acc/);
  });

  it('ключ `Type` (любой регистр) тоже получает акцент', () => {
    render(<PropertiesTable fields={[{ key: 'Type', values: ['note'] }]} />);
    expect(screen.getByText('note').className).toMatch(/acc/);
  });

  it('теги остаются sage-чипами-кнопками; клик вызывает onOpenTag (lowercase, без #)', () => {
    const onOpenTag = vi.fn();
    render(<PropertiesTable fields={FIELDS} onOpenTag={onOpenTag} />);
    const chip = screen.getByRole('button', { name: '#мышление' });
    expect(chip.className).toMatch(/tag/);
    fireEvent.click(chip);
    expect(onOpenTag).toHaveBeenCalledWith('мышление');
  });

  it('тег-чип кликается с клавиатуры (Enter)', () => {
    const onOpenTag = vi.fn();
    render(<PropertiesTable fields={[{ key: 'tags', values: ['Work'] }]} onOpenTag={onOpenTag} />);
    fireEvent.keyDown(screen.getByRole('button', { name: '#Work' }), { key: 'Enter' });
    expect(onOpenTag).toHaveBeenCalledWith('work');
  });

  it('без onOpenTag тег-чип не кнопка (честно, без role=button)', () => {
    render(<PropertiesTable fields={[{ key: 'tags', values: ['solo'] }]} />);
    expect(screen.queryByRole('button')).toBeNull();
    expect(screen.getByText('#solo').className).toMatch(/tag/);
  });

  it('ключ/значение — прямые дети grid-контейнера .properties (без обёртки .propRow)', () => {
    const { container } = render(<PropertiesTable fields={FIELDS} />);
    const grid = container.querySelector('[class*=properties]');
    expect(grid).not.toBeNull();
    // Нет промежуточной строки-обёртки: первый ребёнок — это .propKey (а не .propRow).
    expect(container.querySelector('[class*=propRow]')).toBeNull();
    const firstChild = grid?.firstElementChild;
    expect(firstChild?.className).toMatch(/propKey/);
  });
});
