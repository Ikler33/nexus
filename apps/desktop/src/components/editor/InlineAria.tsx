import { useTranslation } from 'react-i18next';

import { useInlineStore } from '../../stores/inline';

/** Визуально скрыто, но доступно скринридерам. */
const srOnly: React.CSSProperties = {
  position: 'absolute',
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: 'hidden',
  clip: 'rect(0,0,0,0)',
  whiteSpace: 'nowrap',
  border: 0,
};

/**
 * Живой регион для inline-LLM (IL-3, AC-IL-10): анонсирует статус генерации/готовности/ошибки скрин-
 * ридеру (`aria-live="polite"`), не дублируя по символам. Сам ghost — визуальный (`aria-hidden`).
 */
export function InlineAria() {
  const { t } = useTranslation();
  const active = useInlineStore((s) => s.active);
  const streaming = useInlineStore((s) => s.streaming);
  const error = useInlineStore((s) => s.error);

  const message = error
    ? error
    : active && streaming
      ? t('inline.generating')
      : active && !streaming
        ? t('inline.ready')
        : '';

  return (
    <div aria-live="polite" aria-atomic="true" style={srOnly}>
      {message}
    </div>
  );
}
