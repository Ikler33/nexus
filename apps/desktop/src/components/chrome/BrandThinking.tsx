/**
 * «Думающий» бренд-знак (DP-0/DP-1, макет `icons.jsx`): созвездие логотипа с пульсирующими
 * узлами/рёбрами — индикатор работы модели (AI-карточки HOME, генерация инсайтов). Анимации —
 * глобальные классы `src/motion.css` (`brand-thinking`, `bt-node`, `bt-edge` + reduced-motion).
 */
export function BrandThinking({ size = 24 }: { size?: number }) {
  return (
    <svg
      className="brand-thinking"
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      aria-hidden
    >
      <g stroke="currentColor" strokeWidth="1.7" strokeLinecap="round">
        <line className="bt-edge e1" x1="16" y1="16" x2="8" y2="8" />
        <line className="bt-edge e2" x1="16" y1="16" x2="25" y2="11" />
        <line className="bt-edge e3" x1="16" y1="16" x2="12" y2="25" />
      </g>
      <g fill="currentColor">
        <circle className="bt-node n0" cx="16" cy="16" r="2.6" />
        <circle className="bt-node n1" cx="8" cy="8" r="2" />
        <circle className="bt-node n2" cx="25" cy="11" r="2" />
        <circle className="bt-node n3" cx="12" cy="25" r="2" />
      </g>
    </svg>
  );
}
