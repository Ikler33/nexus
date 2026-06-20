import styles from './BrandMark.module.css';

/**
 * Бренд-марк Qasr: «крепость из узлов» (citadel built of the knowledge graph) — треугольная
 * цитадель + узлы графа внутри терракотового squircle. Инлайн-SVG, белый на акцентном фоне.
 * Портирован из дизайн-хендоффа Qasr (icons.jsx BrandMark «fortress from nodes»).
 */
export function BrandMark({ size = 24 }: { size?: number }) {
  return (
    <span className={styles.mark} style={{ width: size, height: size, borderRadius: size * 0.29 }}>
      <svg viewBox="0 0 32 32" width={size * 0.74} height={size * 0.74} fill="none" aria-hidden>
        {/* triangular citadel */}
        <path
          d="M9 23 L16 8 L23 23 Z"
          stroke="#fff"
          strokeWidth={1.7}
          strokeLinecap="round"
          strokeLinejoin="round"
          opacity={0.92}
        />
        {/* graph edges to the core */}
        <g stroke="#fff" strokeWidth={1.5} strokeLinecap="round" opacity={0.6}>
          <line x1={9} y1={23} x2={16} y2={17} />
          <line x1={23} y1={23} x2={16} y2={17} />
        </g>
        {/* nodes */}
        <g fill="#fff">
          <circle cx={16} cy={8} r={2.7} />
          <circle cx={9} cy={23} r={2.2} />
          <circle cx={23} cy={23} r={2.2} />
          <circle cx={16} cy={17} r={2} />
        </g>
      </svg>
    </span>
  );
}
