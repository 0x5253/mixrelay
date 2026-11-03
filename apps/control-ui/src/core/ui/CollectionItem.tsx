import styles from './CollectionItem.module.css';
import { Tooltip } from './Tooltip';

export function CollectionItem({
  name,
  selected,
  disabled,
  removeLabel,
  onSelect,
  onRemove,
}: {
  name: string;
  selected: boolean;
  disabled: boolean;
  removeLabel: string;
  onSelect: () => void;
  onRemove: () => void;
}) {
  return (
    <fieldset className={styles.row} aria-label={name}>
      <button
        type="button"
        className={styles.select}
        aria-current={selected ? 'page' : undefined}
        disabled={disabled || selected}
        onClick={onSelect}
      >
        {name}
      </button>
      <Tooltip label={`${name}: ${removeLabel}`}>
        <button
          type="button"
          className={styles.remove}
          aria-label={`${name}: ${removeLabel}`}
          disabled={disabled}
          onClick={onRemove}
        >
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <path d="M3 4h10M6 4V2h4v2M4 4l.6 10h6.8L12 4M6.5 6.5v5M9.5 6.5v5" />
          </svg>
        </button>
      </Tooltip>
    </fieldset>
  );
}
