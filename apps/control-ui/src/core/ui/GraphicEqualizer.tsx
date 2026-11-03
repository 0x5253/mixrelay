import { type KeyboardEvent, type ReactNode, useState } from 'react';
import {
  equalizerPlot,
  frequencyPosition,
  gainPosition,
  plotFrequency,
  plotGain,
  responsePath,
} from './equalizerResponse';
import styles from './GraphicEqualizer.module.css';
import { NumericValueControl } from './NumericValueControl';
import type { ParameterFieldControl } from './ParameterField';
import { useCapturedPointerDrag } from './useCapturedPointerDrag';

export interface GraphicEqualizerContract {
  bands: readonly { id: string; label: string; frequencyId: string; gainId: string; qId: string }[];
  sampleRate?: number;
}
interface Props {
  renderValues?: (values: ReactNode) => ReactNode;
  contract: GraphicEqualizerContract;
  controls: readonly ParameterFieldControl[];
  disabled?: boolean;
  bypassed?: boolean;
  onChange: (id: string, value: number) => void;
  onParametersChange?: (updates: readonly { id: string; value: number }[]) => void;
}
interface ResolvedBand {
  id: string;
  label: string;
  frequency: ParameterFieldControl;
  gain: ParameterFieldControl;
  q: ParameterFieldControl;
}
function resolveBands(props: Props): ResolvedBand[] {
  return props.contract.bands.flatMap((band) => {
    const frequency = props.controls.find((control) => control.id === band.frequencyId);
    const gain = props.controls.find((control) => control.id === band.gainId);
    const q = props.controls.find((control) => control.id === band.qId);
    return frequency && gain && q ? [{ id: band.id, label: band.label, frequency, gain, q }] : [];
  });
}
/** Dragging a band's handle moves its frequency and gain together, as one batch. */
function useGraphDrag(props: Props, bands: ResolvedBand[], select: (id: string) => void) {
  return useCapturedPointerDrag<ResolvedBand>({
    disabled: props.disabled,
    begin(event) {
      if (!props.onParametersChange) return undefined;
      // A pointer event on the plot targets the plot or an element inside it, never text.
      const target = (event.target as Element).closest<HTMLElement>('[data-band]');
      const band = bands.find((item) => item.id === target?.dataset.band);
      if (!band || !target) return undefined;
      event.preventDefault();
      select(band.id);
      target.focus();
      return band;
    },
    update(band, event) {
      const bounds = event.currentTarget.getBoundingClientRect();
      props.onParametersChange?.([
        {
          id: band.frequency.id,
          value: plotFrequency(
            ((event.clientX - bounds.left) / bounds.width) * equalizerPlot.width,
          ),
        },
        {
          id: band.gain.id,
          value: plotGain(((event.clientY - bounds.top) / bounds.height) * equalizerPlot.height),
        },
      ]);
    },
  });
}
function handleBandKey(event: KeyboardEvent<HTMLButtonElement>, props: Props, band: ResolvedBand) {
  if (props.disabled) return;
  const changes: Record<string, [ParameterFieldControl, number]> = {
    ArrowLeft: [band.frequency, band.frequency.value - band.frequency.step],
    ArrowRight: [band.frequency, band.frequency.value + band.frequency.step],
    ArrowUp: [band.gain, band.gain.value + band.gain.step],
    ArrowDown: [band.gain, band.gain.value - band.gain.step],
  };
  const change = changes[event.key];
  if (!change) return;
  event.preventDefault();
  event.stopPropagation();
  const [control, value] = change;
  props.onChange(
    control.id,
    Math.max(control.minimum, Math.min(control.maximum, Number(value.toPrecision(12)))),
  );
}

/** Graphic base face with nine equivalent value editors composed through the shared node disclosure. */
export function GraphicEqualizer(props: Props) {
  const bands = resolveBands(props);
  const [selected, setSelected] = useState('');
  const selectedBand = bands.find((band) => band.id === selected) ?? bands[0];
  const drag = useGraphDrag(props, bands, setSelected);
  if (!selectedBand) return null;
  const values = (
    <BandSummary {...props} bands={bands} selected={selectedBand.id} select={setSelected} />
  );
  return (
    <div
      className={`${styles.root} nodrag nopan nowheel nokey`}
      data-bypassed={props.bypassed || undefined}
    >
      <div className={styles.plotFrame} {...drag}>
        <svg className={styles.plot} viewBox="0 0 300 120" aria-hidden="true">
          <PlotGrid />
          <path
            className={styles.curve}
            d={responsePath(
              bands.map((band) => ({
                frequency: band.frequency.value,
                gain: band.gain.value,
                q: band.q.value,
              })),
              props.contract.sampleRate ?? 48000,
            )}
          />
          {bands.map((band, index) => (
            <g key={band.id} data-selected={selectedBand.id === band.id}>
              <circle
                className={styles.handle}
                cx={frequencyPosition(band.frequency.value)}
                cy={gainPosition(band.gain.value)}
                r="5"
              />
              <text
                className={styles.bandNumber}
                x={frequencyPosition(band.frequency.value)}
                y={gainPosition(band.gain.value) + 3}
              >
                {index + 1}
              </text>
            </g>
          ))}
        </svg>
        {bands.map((band, index) => (
          <button
            key={band.id}
            type="button"
            className={styles.handleButton}
            data-band={band.id}
            disabled={props.disabled}
            aria-pressed={selectedBand.id === band.id}
            aria-label={`${index + 1} ${band.label}, ${band.frequency.value} Hz, ${band.gain.value} dB, Q ${band.q.value}`}
            style={{
              left: `${(frequencyPosition(band.frequency.value) / equalizerPlot.width) * 100}%`,
              top: `${(gainPosition(band.gain.value) / equalizerPlot.height) * 100}%`,
            }}
            onFocus={() => setSelected(band.id)}
            onClick={() => setSelected(band.id)}
            onKeyDown={(event) => handleBandKey(event, props, band)}
          />
        ))}
      </div>
      {props.renderValues ? props.renderValues(values) : values}
    </div>
  );
}
function BandSummary({
  bands,
  disabled,
  selected,
  select,
  onChange,
}: Props & {
  bands: ResolvedBand[];
  selected: string;
  select: (id: string) => void;
}) {
  return (
    <div className={styles.summary}>
      <div className={styles.summaryLabels} aria-hidden="true">
        <span />
        <span>Hz</span>
        <span>dB</span>
        <span>Q</span>
      </div>
      {bands.map((band, index) => (
        <div className={styles.summaryRow} key={band.id} data-selected={band.id === selected}>
          <button
            type="button"
            className={styles.bandSelect}
            disabled={disabled}
            aria-pressed={band.id === selected}
            aria-label={`${index + 1} ${band.label}`}
            onClick={() => select(band.id)}
          >
            <span>{index + 1}</span>
            <span className={styles.bandLabel}>{band.label}</span>
          </button>
          {[band.frequency, band.gain, band.q].map((control, column) => (
            <NumericValueControl
              key={control.id}
              label={`${index + 1} ${band.label} ${['Hz', 'dB', 'Q'][column]}`}
              value={control.value}
              minimum={control.minimum}
              maximum={control.maximum}
              step={control.step}
              scale={control.scale}
              disabled={disabled}
              onActivate={() => select(band.id)}
              onChange={(value) => onChange(control.id, value)}
            />
          ))}
        </div>
      ))}
    </div>
  );
}
function PlotGrid() {
  return (
    <>
      {[-24, -12, 0, 12, 24].map((gain) => (
        <g key={gain}>
          <path
            className={gain === 0 ? styles.zero : styles.grid}
            d={`M${equalizerPlot.left} ${gainPosition(gain)}H${equalizerPlot.right}`}
          />
          <text className={styles.axis} x="22" y={gainPosition(gain) + 3} textAnchor="end">
            {gain > 0 ? '+' : ''}
            {gain}
          </text>
        </g>
      ))}
      {[20, 100, 1000, 5000, 20000].map((frequency) => (
        <g key={frequency}>
          <path
            className={styles.grid}
            d={`M${frequencyPosition(frequency)} ${equalizerPlot.top}V${equalizerPlot.bottom}`}
          />
          <text
            className={styles.axis}
            x={frequencyPosition(frequency)}
            y="116"
            textAnchor={frequency === 20 ? 'start' : frequency === 20000 ? 'end' : 'middle'}
          >
            {frequency >= 1000 ? `${frequency / 1000}k` : frequency}
          </text>
        </g>
      ))}
    </>
  );
}
