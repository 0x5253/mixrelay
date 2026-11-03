import iconStyles from './IconActionButton.module.css';
import { Tooltip } from './Tooltip';

/**
 * Carbon's header menu button: the first header item, shown only while the navigation
 * collapses (narrow windows). The icon is a menu when closed and a close mark when open.
 */
export function SidebarToggle({
  label,
  opened,
  controls,
  onToggle,
}: {
  label: string;
  opened: boolean;
  controls: string;
  onToggle: () => void;
}) {
  return (
    <Tooltip label={label}>
      <button
        className={iconStyles.root}
        type="button"
        aria-label={label}
        aria-expanded={opened}
        aria-controls={controls}
        onClick={onToggle}
      >
        <svg aria-hidden="true" viewBox="0 0 16 16">
          <path d={opened ? 'm4 4 8 8M12 4l-8 8' : 'M3 4.5h10M3 8h10M3 11.5h10'} />
        </svg>
      </button>
    </Tooltip>
  );
}
