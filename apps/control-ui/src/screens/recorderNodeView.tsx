import type { RecorderNodeInputs } from '../core/ports/control/RecorderNodeView';
import { RecorderControls } from '../features/recorder/RecorderControls';

export function renderRecorderNode(input: RecorderNodeInputs) {
  return <RecorderControls key={`${input.graphId}:${input.nodeId}`} {...input} />;
}
