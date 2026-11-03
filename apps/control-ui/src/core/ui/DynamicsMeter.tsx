import type { CSSProperties, ReactNode } from 'react';
import { useDynamicsMeterReading } from '../audio-meter/useDynamicsMeterReading';
import { meterScale } from '../theme/tokens';
import styles from './DynamicsMeter.module.css';
import type { ParameterFieldControl } from './ParameterField';

export interface DynamicsMeterContract {
  nodeId: string;
  portId: string;
  thresholdId: string;
  inputLabel: string;
  outputLabel: string;
  reductionLabel: string;
  unavailableLabel: string;
  channels?: number;
}
interface Props {
  contract: DynamicsMeterContract;
  controls: readonly ParameterFieldControl[];
  disabled?: boolean;
  bypassed?: boolean;
}
const floorDb = meterScale.floorDb;
const position = (db: number) => Math.max(0, Math.min(100, ((db - floorDb) / -floorDb) * 100));
const decibels = (peak: number) => (peak > 0 ? 20 * Math.log10(peak) : Number.NEGATIVE_INFINITY);
const dbText = (db: number) => `${db === Number.NEGATIVE_INFINITY ? '−∞' : db.toFixed(1)} dB`;

/** Input/output peaks and actual pre-makeup attenuation; no peak-difference inference. */
export function DynamicsMeter({ contract, controls, disabled, bypassed }: Props) {
  const reading = useDynamicsMeterReading(contract.nodeId, contract.portId);
  const threshold = controls.find((control) => control.id === contract.thresholdId);
  if (!threshold) return null;
  return (
    <div
      className={styles.root}
      data-disabled={disabled || undefined}
      data-bypassed={bypassed || undefined}
    >
      <div className={styles.banks}>
        <PeakBank
          label={contract.inputLabel}
          peaks={reading?.inputPeaks ?? Array.from({ length: contract.channels ?? 2 }, () => null)}
          threshold={reading?.thresholdDb ?? threshold.value}
          unavailableLabel={contract.unavailableLabel}
        />
        <Reduction
          label={contract.reductionLabel}
          value={reading?.gainReductionDb ?? null}
          unavailableLabel={contract.unavailableLabel}
        />
        <PeakBank
          label={contract.outputLabel}
          peaks={reading?.outputPeaks ?? Array.from({ length: contract.channels ?? 2 }, () => null)}
          unavailableLabel={contract.unavailableLabel}
        />
      </div>
      <span className={styles.thresholdValue}>
        {threshold.label} {dbText(reading?.thresholdDb ?? threshold.value)}
      </span>
    </div>
  );
}

function PeakBank({
  label,
  peaks,
  threshold,
  unavailableLabel,
}: {
  label: string;
  peaks: readonly (number | null)[];
  threshold?: number;
  unavailableLabel: string;
}) {
  return (
    <div className={styles.bank}>
      <span className={styles.label}>{label}</span>
      <div className={styles.rails}>
        {peaks.map((peak, index) => {
          const db = peak === null ? null : decibels(peak);
          const channel = peaks.length === 2 ? ['L', 'R'][index] : String(index + 1);
          return (
            <div className={styles.channel} key={channel}>
              <span className={styles.channelName}>{channel}</span>
              <ReadingRail
                label={`${label} ${channel}`}
                value={db}
                minimum={floorDb}
                maximum={0}
                unavailableLabel={unavailableLabel}
              >
                <span
                  className={styles.fill}
                  style={
                    {
                      '--meter-unfilled': `${100 - (db === null ? 0 : position(db))}%`,
                    } as CSSProperties
                  }
                />
                {threshold !== undefined ? (
                  <span
                    className={styles.threshold}
                    data-threshold-db={threshold}
                    style={{ left: `${position(threshold)}%` }}
                    aria-hidden="true"
                  />
                ) : null}
              </ReadingRail>
              <span className={styles.value}>{db === null ? '—' : dbText(db)}</span>
            </div>
          );
        })}
        <Scale minimum="−60" middle="−30" maximum="0" />
      </div>
    </div>
  );
}

function Reduction({
  label,
  value,
  unavailableLabel,
}: {
  label: string;
  value: number | null;
  unavailableLabel: string;
}) {
  return (
    <div className={styles.bank}>
      <span className={styles.label}>{label}</span>
      <div className={styles.rails}>
        <div className={styles.channel}>
          <span />
          <ReadingRail
            label={label}
            value={value}
            minimum={0}
            maximum={24}
            unavailableLabel={unavailableLabel}
            reduction
          >
            <span
              className={styles.fill}
              style={
                {
                  '--layout-dynamics-level': `${value === null ? 0 : Math.min(100, (value / 24) * 100)}%`,
                } as CSSProperties
              }
            />
          </ReadingRail>
          <span className={styles.value}>{value === null ? '—' : `${value.toFixed(1)} dB`}</span>
        </div>
        <Scale minimum="0" middle="12" maximum="24" />
      </div>
    </div>
  );
}

function Scale({ minimum, middle, maximum }: { minimum: string; middle: string; maximum: string }) {
  return (
    <div className={styles.channel} aria-hidden="true">
      <span />
      <span className={styles.scale}>
        <span>{minimum}</span>
        <span>{middle}</span>
        <span>{maximum}</span>
      </span>
      <span />
    </div>
  );
}

function ReadingRail({
  label,
  value,
  minimum,
  maximum,
  unavailableLabel,
  reduction,
  children,
}: {
  label: string;
  value: number | null;
  minimum: number;
  maximum: number;
  unavailableLabel: string;
  reduction?: boolean;
  children: ReactNode;
}) {
  const className = reduction ? `${styles.rail} ${styles.reduction}` : styles.rail;
  if (value === null)
    return (
      <span className={className} role="img" aria-label={`${label}: ${unavailableLabel}`}>
        {children}
      </span>
    );
  return (
    // biome-ignore lint/a11y/useSemanticElements: Custom audio rail owns read-only dB meter semantics.
    <span
      className={className}
      role="meter"
      aria-label={label}
      aria-valuemin={minimum}
      aria-valuemax={maximum}
      aria-valuenow={Math.max(minimum, Math.min(maximum, value))}
      aria-valuetext={dbText(value)}
    >
      {children}
    </span>
  );
}
