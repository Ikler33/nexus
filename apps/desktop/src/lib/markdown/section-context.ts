import { createContext } from 'react';

/**
 * Контекст сворачивания H2-секций (Hermes-8 S3). Состояние (`Set` свёрнутых `data-sec-id`) и тоггл живут
 * в MarkdownPreview, а потребляют их РАЗНЫЕ component-оверрайды react-markdown (`h2` — кнопка+шеврон,
 * `section` — класс `.collapsed` на обёртке). Прокинуть пропсами нельзя (react-markdown сам инстанцирует
 * оверрайды и форвардит им только `node`/`children`), поэтому — через контекст.
 *
 * Дефолт — пустой: вне MarkdownPreview (нет провайдера) `isCollapsed` всегда false, `toggle` — no-op
 * (секции не сворачиваются, но и не падают — безопасно для любого изолированного рендера h2/section).
 */
export type SectionState = {
  isCollapsed: (secId: string) => boolean;
  toggle: (secId: string) => void;
};

export const SectionContext = createContext<SectionState>({
  isCollapsed: () => false,
  toggle: () => {},
});
