import type { Ref } from 'react';
import type { AudioNodeCardProps } from './AudioNodeCard';
import styles from './AudioNodeCard.module.css';
import { AudioNodeIcon } from './AudioNodeIcon';
import { AudioNodeMeterBanks } from './AudioNodeMeterBanks';
import { compactFeedbackMessage, ioSummaryText } from './audioNodeCardModel';
import type { AudioNodeFaceMode } from './audioNodeLayout';
import { DeviceAppActivity } from './DeviceAppActivity';
import { ProcessorTimingLabel } from './ProcessorTimingLabel';
import { StatusIndicator } from './StatusIndicator';
import { Text } from './Text';

type AudioNodeFaceProps = Pick<
  AudioNodeCardProps,
  | 'category'
  | 'processingTimeNs'
  | 'channelCount'
  | 'detail'
  | 'faceMode'
  | 'deviceAppActivity'
  | 'icon'
  | 'iconUrl'
  | 'inputCount'
  | 'meterBanks'
  | 'onSelect'
  | 'offline'
  | 'outputCount'
  | 'selected'
  | 'status'
  | 'statusTone'
  | 'title'
> & { hideSummary?: boolean; faceRef?: (element: HTMLElement | null) => void };

/** The node's identity face: title, status, meters and channel summary. It is the
 * selection button only for a standalone card; a graph owns selection itself. */
export function AudioNodeFace(props: AudioNodeFaceProps) {
  const {
    deviceAppActivity,
    processingTimeNs,
    category,
    channelCount = 0,
    hideSummary,
    detail,
    icon,
    iconUrl,
    inputCount,
    meterBanks = [],
    faceMode = 'standard',
    onSelect,
    offline,
    outputCount,
    selected,
    status,
    statusTone,
    title,
  } = props;
  const Face = onSelect ? 'button' : 'div';
  return (
    <Face
      type={onSelect ? 'button' : undefined}
      ref={props.faceRef as Ref<HTMLButtonElement & HTMLDivElement>}
      data-node-face=""
      aria-label={
        onSelect ? [title, category, detail, status].filter(Boolean).join(', ') : undefined
      }
      aria-description={onSelect ? deviceAppActivity?.summary : undefined}
      aria-pressed={onSelect ? selected : undefined}
      className={styles.button}
      onClick={onSelect}
    >
      <span className={styles.topLine}>
        <span className={styles.icon} aria-hidden="true">
          <AudioNodeIcon icon={icon} iconUrl={iconUrl} />
        </span>
        <span className={styles.category} data-node-title>
          {title}
        </span>
        <ProcessorTimingLabel nanoseconds={processingTimeNs} />
        {status ? (
          <span className={styles.statusSlot}>
            <StatusIndicator label={status} tone={statusTone} position="left" />
          </span>
        ) : null}
      </span>
      <span className={styles.body}>
        {faceMode !== 'compact' && hideSummary && detail ? (
          <small className={styles.detail}>{detail}</small>
        ) : null}
        {faceMode !== 'compact' && meterBanks.length > 0 ? (
          <AudioNodeMeterBanks banks={meterBanks} offline={Boolean(offline)} title={title} />
        ) : null}
      </span>
      {!hideSummary ? (
        <span className={styles.ioSummary}>
          <span aria-hidden="true">{ioSummaryText(inputCount, outputCount, channelCount)}</span>
          {deviceAppActivity ? <DeviceAppActivity {...deviceAppActivity} /> : null}
          {detail ? <small className={styles.detail}>{detail}</small> : null}
        </span>
      ) : null}
    </Face>
  );
}

export function CompactNodeFeedback({
  faceMode,
  detail,
  status,
  statusTone,
}: Pick<AudioNodeCardProps, 'detail' | 'status' | 'statusTone'> & { faceMode: AudioNodeFaceMode }) {
  const message = compactFeedbackMessage(faceMode, detail, status, statusTone);
  return message ? (
    <div className={styles.compactDetail}>
      <Text kind="label" tone={statusTone}>
        {message}
      </Text>
    </div>
  ) : null;
}
