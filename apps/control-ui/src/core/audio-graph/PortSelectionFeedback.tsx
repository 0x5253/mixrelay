import type { CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { i18n } from '../i18n';
import styles from './AudioGraphCanvas.module.css';
import type { AudioGraphModel } from './model';
import type { SelectedAudioPort } from './portSelection';

function channelRanges(channels: readonly number[]) {
  const sorted = [...new Set(channels)].sort((a, b) => a - b);
  const ranges: string[] = [];
  let start = sorted[0];
  let end = start;
  for (const channel of sorted.slice(1)) {
    if (end !== undefined && channel === end + 1) end = channel;
    else {
      ranges.push(start === end ? `${start}` : `${start}–${end}`);
      start = channel;
      end = channel;
    }
  }
  if (start !== undefined) ranges.push(start === end ? `${start}` : `${start}–${end}`);
  return ranges.join(', ');
}

export function PortSelectionFeedback({
  model,
  ports,
  position,
}: {
  model: AudioGraphModel;
  ports: readonly SelectedAudioPort[];
  position: { x: number; y: number } | null;
}) {
  const { t } = useTranslation(undefined, { i18n });
  if (!position || ports.length === 0) return null;
  const node = model.nodes.find((node) => node.id === ports[0]?.nodeId);
  const selectedIds = new Set(ports.map((port) => port.portId));
  const channels =
    node?.outputPorts.flatMap((port, index) =>
      selectedIds.has(port.id) ? [(port.channelIndex ?? index) + 1] : [],
    ) ?? [];
  const ranges = channelRanges(channels);
  if (!ranges) return null;
  return (
    <div
      className={styles.portSelectionFeedback}
      style={
        {
          '--selection-x': `${position.x}px`,
          '--selection-y': `${position.y}px`,
        } as CSSProperties
      }
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      CH {ranges} · {t('routing.processorLayouts.channelCount', { count: channels.length })}
    </div>
  );
}
