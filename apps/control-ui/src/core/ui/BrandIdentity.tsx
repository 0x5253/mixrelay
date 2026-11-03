import symbol from '../../../../../branding/identity/source/symbol.json';
import { brandSizes } from '../theme/tokens';
import styles from './BrandIdentity.module.css';

type BrandIdentityProps = {
  size?: keyof typeof brandSizes;
  tone?: 'primary' | 'neutral' | 'inverse';
  outline?: boolean;
};

/** The approved symbol carries the accessible product name and is never an action. */
export function BrandIdentity({
  size = 'compact',
  tone = 'primary',
  outline = false,
}: BrandIdentityProps) {
  // Line artwork needs room for both contour edges; small product surfaces use fill.
  const actualSize = outline ? 'display' : size;
  const height = brandSizes[actualSize];
  const inset = outline ? height / 76 : 0;
  return (
    <svg
      className={styles.identity}
      data-size={actualSize}
      data-tone={tone}
      data-outline={outline}
      viewBox={`${-inset} ${-inset} ${height + inset * 2} ${height + inset * 2}`}
      width={height + inset * 2}
      height={height + inset * 2}
      role="img"
      aria-label="MixRelay"
      focusable="false"
    >
      <path
        className={styles.symbol}
        d={symbol.path}
        transform={`translate(${(-10 * height) / 76} ${(-10 * height) / 76}) scale(${height / 76})`}
        fill={outline ? 'none' : 'currentColor'}
        stroke={outline ? 'currentColor' : undefined}
        strokeWidth={outline ? symbol.outlineWidth : undefined}
        strokeLinejoin={outline ? 'round' : undefined}
      />
    </svg>
  );
}

export function ProductHeading() {
  return (
    <h1 className={styles.heading}>
      <BrandIdentity />
    </h1>
  );
}
