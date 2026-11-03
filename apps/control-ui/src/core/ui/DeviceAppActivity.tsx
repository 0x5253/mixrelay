import { useEffect, useId, useRef, useState } from 'react';
import { AudioNodeIcon } from './AudioNodeIcon';
import styles from './DeviceAppActivity.module.css';
import { Tooltip } from './Tooltip';

export interface DeviceAppActivityProps {
  direction: 'playing' | 'recording';
  apps: readonly { id: string; label: string; iconUrl?: string }[];
  /** Complete localized list, also supplied to the outer node accessible description. */
  summary: string;
  /** Empty, checking or unavailable wording supplied by Feature. */
  label: string;
  overflowLabel: string;
  overflowDescription: string;
}

/** Readonly utility group. The existing node selection owner supplies keyboard focus. */
export function DeviceAppActivity({
  direction,
  apps,
  summary,
  label,
  overflowLabel,
  overflowDescription,
}: DeviceAppActivityProps) {
  const descriptionId = useId();
  const ref = useRef<HTMLSpanElement>(null);
  // Open while the owning node has focus, until dismissed (Escape) or focus leaves.
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const owner = ref.current?.closest<HTMLElement>('.react-flow__node, button');
    if (!owner) return;
    const focus = () => setShown(true);
    const blur = () => setShown(false);
    owner.addEventListener('focus', focus);
    owner.addEventListener('blur', blur);
    setShown(document.activeElement === owner);
    return () => {
      owner.removeEventListener('focus', focus);
      owner.removeEventListener('blur', blur);
    };
  }, []);
  // Reapplied after every render: the owner may have rewritten its own description list.
  useEffect(() => {
    const owner = ref.current?.closest<HTMLElement>('.react-flow__node, button');
    return owner ? describeOwner(owner, descriptionId) : undefined;
  });
  return (
    <>
      <span hidden id={descriptionId}>
        {summary}
      </span>
      <Tooltip
        label={summary}
        nonInteractive
        opened={shown ? true : undefined}
        onOpenChange={setShown}
      >
        <span ref={ref} className={styles.root} role="img" aria-label={summary}>
          <svg aria-hidden="true" className={styles.direction} viewBox="0 0 16 16">
            {direction === 'playing' ? (
              <path d="M2 8h11M9 4l4 4-4 4" />
            ) : (
              <path d="M14 8H3M7 4L3 8l4 4" />
            )}
          </svg>
          {apps.length ? (
            apps.slice(0, 3).map((app) => (
              <Tooltip key={app.id} label={app.label} nonInteractive>
                <span className={styles.icon} aria-hidden="true">
                  <AudioNodeIcon icon="APP" iconUrl={app.iconUrl} />
                </span>
              </Tooltip>
            ))
          ) : (
            <span className={styles.label}>{label}</span>
          )}
          {apps.length > 3 ? (
            <Tooltip label={overflowDescription} nonInteractive>
              <span className={styles.overflow} aria-hidden="true">
                {overflowLabel}
              </span>
            </Tooltip>
          ) : null}
        </span>
      </Tooltip>
    </>
  );
}

/** Preserve React Flow's keyboard instructions; aria-description alone loses to aria-describedby. */
function describeOwner(owner: HTMLElement, id: string) {
  const descriptions = () =>
    (owner.getAttribute('aria-describedby') ?? '').split(' ').filter(Boolean);
  owner.setAttribute('aria-describedby', [...new Set([...descriptions(), id])].join(' '));
  return () => {
    const remaining = descriptions()
      .filter((entry) => entry !== id)
      .join(' ');
    if (remaining) owner.setAttribute('aria-describedby', remaining);
    else owner.removeAttribute('aria-describedby');
  };
}
