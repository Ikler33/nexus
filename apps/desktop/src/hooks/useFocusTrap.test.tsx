import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { useFocusTrap } from './useFocusTrap';

function Harness({ onClose }: { onClose: () => void }) {
  const ref = useFocusTrap<HTMLDivElement>(onClose);
  return (
    <div ref={ref} tabIndex={-1} role="dialog">
      <button>first</button>
      <button>mid</button>
      <button>last</button>
    </div>
  );
}

describe('useFocusTrap (P9)', () => {
  it('переводит фокус на первый фокусируемый при монтировании', () => {
    render(<Harness onClose={() => {}} />);
    expect(screen.getByText('first')).toHaveFocus();
  });

  it('Esc вызывает onClose', () => {
    const onClose = vi.fn();
    render(<Harness onClose={onClose} />);
    fireEvent.keyDown(screen.getByText('first'), { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('Tab с последнего элемента циклит на первый', () => {
    render(<Harness onClose={() => {}} />);
    const last = screen.getByText('last');
    last.focus();
    fireEvent.keyDown(last, { key: 'Tab' });
    expect(screen.getByText('first')).toHaveFocus();
  });

  it('Shift+Tab с первого элемента циклит на последний', () => {
    render(<Harness onClose={() => {}} />);
    const first = screen.getByText('first');
    first.focus();
    fireEvent.keyDown(first, { key: 'Tab', shiftKey: true });
    expect(screen.getByText('last')).toHaveFocus();
  });
});
