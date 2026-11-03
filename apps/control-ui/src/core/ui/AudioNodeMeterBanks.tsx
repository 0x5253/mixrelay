import { useTranslation } from 'react-i18next';
import { useAudioMeterLevels } from '../audio-meter/useAudioMeterLevels';
import styles from './AudioNodeCard.module.css';
import { Meter } from './Meter';

export interface AudioNodeMeterBank {
  channels: number;
  label?: string;
  nodeId: string;
  portId: string;
}

export function AudioNodeMeterBanks({
  banks,
  offline,
  title,
}: {
  banks: readonly AudioNodeMeterBank[];
  offline: boolean;
  title: string;
}) {
  const visibleBanks = banks.filter((bank) => bank.channels > 0);
  if (visibleBanks.length === 0) return null;
  return (
    <span className={styles.meterGroups}>
      {visibleBanks.map((bank) => (
        <MeterBank
          bank={bank}
          key={JSON.stringify([bank.nodeId, bank.portId])}
          offline={offline}
          showLabel={visibleBanks.length > 1}
          title={title}
        />
      ))}
    </span>
  );
}

function MeterBank({
  bank,
  offline,
  showLabel,
  title,
}: {
  bank: AudioNodeMeterBank;
  offline: boolean;
  showLabel: boolean;
  title: string;
}) {
  const levels = useAudioMeterLevels(bank.nodeId, bank.portId);
  return (
    <span className={styles.meterGroup} data-meter-port={bank.portId}>
      {showLabel && bank.label ? <span className={styles.meterBankLabel}>{bank.label}</span> : null}
      <NodeMeters
        channels={bank.channels}
        levels={offline ? [] : levels}
        meterLabel={bank.label ? `${title} ${bank.label}` : title}
      />
    </span>
  );
}

function NodeMeters({
  channels,
  levels,
  meterLabel,
}: {
  channels: number;
  levels: readonly number[];
  meterLabel: string;
}) {
  const { t } = useTranslation();
  const channelLabel = (channel: number) =>
    t('routing.a11y.meterChannel', { name: meterLabel, channel });
  const visibleChannels = Math.min(64, Math.max(1, Math.trunc(channels)));
  const channelNumbers = Array.from({ length: visibleChannels }, (_, index) => index + 1);
  if (visibleChannels > 8) {
    return (
      <span
        className={styles.meterBank}
        data-channels={visibleChannels}
        data-density={visibleChannels > 32 ? 'dense' : 'compact'}
      >
        {channelNumbers.map((channel) => (
          <span className={styles.meterCell} key={`channel-${channel}`}>
            <Meter
              label={channelLabel(channel)}
              level={levels[channel - 1] ?? 0}
              orientation="vertical"
            />
            <span aria-hidden="true" className={styles.meterIndex}>
              {meterIndexLabel(channel, visibleChannels)}
            </span>
          </span>
        ))}
      </span>
    );
  }
  return (
    <span className={styles.meters} data-channels={visibleChannels}>
      {channelNumbers.map((channel) => (
        <span className={styles.meterRow} key={`channel-${channel}`}>
          <span aria-hidden="true">{meterChannelLabel(channel, visibleChannels)}</span>
          <Meter
            label={channelLabel(channel)}
            level={levels[channel - 1] ?? 0}
            orientation="horizontal"
          />
        </span>
      ))}
    </span>
  );
}

function meterChannelLabel(channel: number, channels: number) {
  if (channels === 2) return channel === 1 ? 'L' : 'R';
  return `${channel}`;
}

function meterIndexLabel(channel: number, channels: number) {
  return channel === 1 || channel === channels || channel % 8 === 0 ? channel : '';
}
