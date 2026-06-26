import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { formatCombo } from '../../lib/commands';
import { Titlebar } from './Titlebar';

describe('Titlebar — пилюля палитры (P1-20 honesty)', () => {
  // Регресс-страховка от over-reporting: подсказка-шорткат поисковой пилюли ДОЛЖНА совпадать с тем,
  // что реально открывает палитру. Палитра — `mod+p` (commands-core.ts: openPalette); `mod+k` занят
  // `editor.format.link` (вставка ссылки) → старая хардкод-подсказка «⌘K» врала (нажатие не открывало
  // палитру). Теперь рендерим formatCombo('mod+p') (локаль-корректно: ⌘P на mac / Ctrl+P иначе).
  it('kbd-подсказка = реальный шорткат палитры (mod+p), НЕ врёт ⌘K', () => {
    const { container } = render(<Titlebar />);
    const kbd = container.querySelector('kbd');
    expect(kbd).not.toBeNull();
    expect(kbd!.textContent).toBe(formatCombo('mod+p'));
    // ⌘K = editor.format.link, не палитра → подсказка не должна быть «K»-комбинацией
    expect(kbd!.textContent).not.toContain('K');
  });
});
