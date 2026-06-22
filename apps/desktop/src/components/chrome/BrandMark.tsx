import styles from './BrandMark.module.css';

/**
 * Бренд-знак Orvin: «узел на орбите» — кольцо с РАЗРЫВОМ под ember-точкой, спутник
 * СИДИТ на орбите (точка на пути), а не камень над сплошным кольцом. Tile-less: кольцо
 * в currentColor (подхватывает цвет текста титлбара/онбординга), спутник — Ember
 * (`--color-accent`, единственная тёплая точка в UI). Геометрия из дизайн-хэндоффа
 * Hermes-6 (`icons.jsx` BrandMark «node on the orbit», viewBox 32×32). App-icon-форма
 * (белый знак на ember-плитке) — отдельная поверхность (favicon/app-icons), не здесь.
 */
export function BrandMark({ size = 24 }: { size?: number }) {
  return (
    <span className={styles.mark} style={{ width: size, height: size }}>
      <svg viewBox="0 0 32 32" width={size} height={size} fill="none" aria-hidden>
        <path
          d="M24.61 11.98 A9.5 9.5 0 1 1 20.02 7.39"
          stroke="currentColor"
          strokeWidth={2.4}
          fill="none"
          strokeLinecap="round"
        />
        <circle cx={22.72} cy={9.28} r={3} fill="var(--color-accent)" />
      </svg>
    </span>
  );
}
