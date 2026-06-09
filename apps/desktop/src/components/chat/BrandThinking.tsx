import styles from './BrandThinking.module.css';

/**
 * Анимированный «созвездие»-логотип — индикатор размышления модели (DESIGN `icons.jsx`/`motion.css`
 * §brand-thinking): дышит, узлы и рёбра пульсируют в акцентном цвете. Рядом с ним — переливающийся
 * label (живая сводка размышления). Замирает при `prefers-reduced-motion`.
 */
export function BrandThinking({ size = 28 }: { size?: number }) {
  return (
    <svg
      viewBox="0 0 32 32"
      width={size}
      height={size}
      className={styles.mark}
      fill="none"
      aria-hidden
    >
      <g stroke="currentColor" strokeWidth={1.7} strokeLinecap="round">
        <line className={`${styles.edge} ${styles.e0}`} x1={16} y1={16} x2={8} y2={8} />
        <line className={`${styles.edge} ${styles.e1}`} x1={16} y1={16} x2={25} y2={11} />
        <line className={`${styles.edge} ${styles.e2}`} x1={16} y1={16} x2={12} y2={25} />
        <line className={`${styles.edge} ${styles.e3}`} x1={25} y1={11} x2={12} y2={25} />
      </g>
      <g fill="currentColor">
        <circle className={`${styles.node} ${styles.n0}`} cx={16} cy={16} r={3.4} />
        <circle className={`${styles.node} ${styles.n1}`} cx={8} cy={8} r={2.5} />
        <circle className={`${styles.node} ${styles.n2}`} cx={25} cy={11} r={2.5} />
        <circle className={`${styles.node} ${styles.n3}`} cx={12} cy={25} r={2.5} />
      </g>
    </svg>
  );
}
