/**
 * «Думающий» бренд-знак Orvin (Hermes-6 `icons.jsx` BrandThinking): орбита в движении —
 * ember-спутник облетает кольцо, пока работает модель. Индикатор ЛЮБОГО AI-«работает»
 * состояния (чат, резюме, похожие, дайджест, инсайты, HOME-карточки). Анимация — глобальные
 * классы `src/motion.css` (`bt-orbit`/`bt-ring`/`bt-sat` + reduced-motion). Вариант покоя для
 * empty-state (медленный дрейф, тусклое кольцо) — передать `className="idle"`.
 */
export function BrandThinking({
  size = 24,
  className = '',
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      className={`brand-thinking ${className}`.trim()}
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      aria-hidden
    >
      <g className="bt-orbit">
        <path
          className="bt-ring"
          d="M24.15 12.19 A9 9 0 1 1 19.81 7.85"
          stroke="currentColor"
          strokeWidth="2"
          fill="none"
          strokeLinecap="round"
        />
        <circle className="bt-sat" cx="22.36" cy="9.64" r="2.6" fill="currentColor" />
      </g>
    </svg>
  );
}
