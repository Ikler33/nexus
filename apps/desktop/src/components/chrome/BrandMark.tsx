import styles from './BrandMark.module.css';

/**
 * Бренд-марк Nexus: «созвездие» из 4 связанных узлов (граф знаний) внутри терракотового
 * squircle. Инлайн-SVG, `currentColor`-независимый (белый на акцентном фоне). См. дизайн-хендофф.
 */
export function BrandMark({ size = 24 }: { size?: number }) {
  return (
    <span className={styles.mark} style={{ width: size, height: size, borderRadius: size * 0.29 }}>
      <svg viewBox="0 0 32 32" width={size * 0.74} height={size * 0.74} fill="none" aria-hidden>
        <g stroke="#fff" strokeWidth={1.7} strokeLinecap="round" opacity={0.92}>
          <line x1={16} y1={16} x2={8} y2={8} />
          <line x1={16} y1={16} x2={25} y2={11} />
          <line x1={16} y1={16} x2={12} y2={25} />
          <line x1={25} y1={11} x2={12} y2={25} />
        </g>
        <g fill="#fff">
          <circle cx={16} cy={16} r={3.4} />
          <circle cx={8} cy={8} r={2.5} />
          <circle cx={25} cy={11} r={2.5} />
          <circle cx={12} cy={25} r={2.5} />
        </g>
      </svg>
    </span>
  );
}
