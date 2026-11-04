import type { ReactNode, Ref } from 'react';
import { Button } from 'react-aria-components';
import styles from './AudioNodeCard.module.css';
import { type AudioNodeMeterBank, AudioNodeMeterBanks } from './AudioNodeMeterBanks';
import type { AudioNodeParameterControl } from './AudioNodeParameterGrid';
import { offersProcessorEditorAction } from './audioNodeCardModel';
import type { ProcessorEditorControl } from './audioNodeCardTypes';
import type { AudioNodeFaceMode } from './audioNodeLayout';
import { DynamicsMeter, type DynamicsMeterContract } from './DynamicsMeter';
import { Tooltip } from './Tooltip';

function NodeContent({ children }: { children: ReactNode }) {
  return <div className={`${styles.content} nodrag nopan nowheel`}>{children}</div>;
}

/** The card's control area below the face: inline content, meters and settings. */
export function AudioNodeControls({
  content,
  compactMeterBanks,
  dynamics,
  parameterControls,
  bypassed,
  faceMode,
  measureRef,
  offline,
  title,
  children,
}: {
  content: ReactNode;
  compactMeterBanks: readonly AudioNodeMeterBank[];
  dynamics?: DynamicsMeterContract;
  parameterControls: readonly AudioNodeParameterControl[];
  bypassed: boolean;
  faceMode: AudioNodeFaceMode;
  measureRef: Ref<HTMLDivElement>;
  offline: boolean;
  title: string;
  children: ReactNode;
}) {
  // DOM content and settings set the minimum height; the graph owns native editor geometry.
  const compact = faceMode === 'compact';
  const dynamicsMeter = dynamics ? (
    <DynamicsMeter
      contract={dynamics}
      controls={parameterControls}
      disabled={offline}
      bypassed={bypassed}
    />
  ) : null;
  if (compact && compactMeterBanks.length > 0) {
    return (
      <div className={styles.controls} data-node-controls="">
        <div ref={measureRef} className={styles.settings}>
          <AudioNodeMeterBanks banks={compactMeterBanks} offline={offline} title={title} />
          {dynamicsMeter}
          {children}
          {content ? <NodeContent>{content}</NodeContent> : null}
        </div>
      </div>
    );
  }
  return (
    <div ref={compact ? undefined : measureRef} className={styles.controls} data-node-controls="">
      {!compact && content ? <NodeContent>{content}</NodeContent> : null}
      <div ref={compact ? measureRef : undefined} className={styles.settings}>
        {compact && content ? <NodeContent>{content}</NodeContent> : null}
        {dynamicsMeter}
        {children}
      </div>
    </div>
  );
}

/** Opens a closed native editor or retries a failed one, and reports the failure. */
export function ProcessorEditorOption({
  title,
  control,
  onRetry,
}: {
  title: string;
  control?: ProcessorEditorControl;
  onRetry?: () => void;
}) {
  if (!control || !onRetry || !offersProcessorEditorAction(control)) return null;
  return (
    <div className={`${styles.processorEditor} nodrag nopan nowheel`}>
      <Tooltip label={`${title}: ${control.actionLabel}`}>
        <Button
          aria-label={`${title}: ${control.actionLabel}`}
          className={styles.processorEditorButton}
          data-state={control.state}
          onClick={(event) => {
            event.stopPropagation();
            onRetry();
          }}
        >
          <svg
            aria-hidden="true"
            width="14"
            height="14"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.25"
          >
            {control.state === 'closed' ? (
              <path d="M6 3H3v10h10v-3M9 3h4v4M13 3 7 9" />
            ) : (
              <path d="M13 7a5 5 0 1 0-1 4M13 2v5H8" />
            )}
          </svg>
        </Button>
      </Tooltip>
      {control.failureLabel ? (
        <span role="status" className={styles.processorEditorFailure}>
          {control.failureLabel}
        </span>
      ) : null}
    </div>
  );
}
