import type { SVGProps } from 'react';

/**
 * Бренд-глифы Orvin/Castor (Hermes-6 `icons.jsx`) — drop-in замена двух lucide-иконок,
 * закрепляющая трёхуровневый визуальный язык: **орбита** = «AI-слой Orvin» (любой
 * AI-афорданс), **комета** = «агент Castor», нейтральный Lucide = всё остальное.
 *
 * API совместим с lucide-react (`size`/`strokeWidth`/`className`/`style` + проброс
 * остальных SVG-пропов), поэтому глифы работают и как `<OrbitIcon size={16} />`, и как
 * ссылка на компонент (`icon={OrbitIcon}`). viewBox 24×24, обводка — currentColor.
 */
type GlyphProps = Omit<SVGProps<SVGSVGElement>, 'ref'> & { size?: number | string };

/** sparkles → орбита: знак AI-слоя Orvin (анализ-меню, дайджест, «Похожие», инсайты, модели…). */
export function OrbitIcon({
  size = 24,
  strokeWidth = 2,
  className,
  style,
  ...rest
}: GlyphProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={{ flexShrink: 0, ...style }}
      {...rest}
    >
      <path d="M18.34 9.04 A7 7 0 1 1 14.96 5.66" />
      <circle cx="16.95" cy="7.05" r="2.3" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** bot → комета Castor: знак агента (нав-бар Агента, командная палитра). */
export function CometIcon({
  size = 24,
  strokeWidth = 2,
  className,
  style,
  ...rest
}: GlyphProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={{ flexShrink: 0, ...style }}
      {...rest}
    >
      <path d="M4 19 Q10 14 14.5 9.5" opacity="0.5" />
      <circle cx="17" cy="7" r="2.6" fill="currentColor" stroke="none" />
    </svg>
  );
}
