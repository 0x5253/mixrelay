import { useUpdateNodeInternals } from '@xyflow/react';
import { useCallback, useEffect, useState } from 'react';

import {
  audioNodeFaceGeometry,
  audioNodeFrame,
  minimumAudioNodeFaceHeight,
} from '../ui/audioNodeLayout';
import type { AudioGraphNode } from './model';

export function graphNodeFaceMode(node: AudioGraphNode) {
  if (['provider', 'effect', 'compressor', 'limiter'].includes(node.runtime?.type ?? ''))
    return 'compact';
  return node.runtime?.type === 'recorder' ? 'channel-rows' : 'standard';
}

export function graphNodeFaceGeometry(node: AudioGraphNode) {
  return audioNodeFaceGeometry({
    meterBanks: node.appearance.meterBanks,
    portCount: Math.max(node.inputPorts.length, node.outputPorts.length),
    mode: graphNodeFaceMode(node),
  });
}

function nativeEditorHeight(
  node: AudioGraphNode,
  editor: { width: number | null; height: number | null },
) {
  if (!editor.width || !editor.height) return 0;
  return (
    (editor.height * (node.size?.width ?? Math.min(1600, Math.max(240, editor.width)))) /
      editor.width +
    28
  );
}

function channelCenter(first: number | null, height: number, minimum: number) {
  return first === null ? null : first + Math.max(0, height - minimum) / 2;
}

export function useAudioNodeLayout(
  node: AudioGraphNode,
  editor: { width: number | null; height: number | null },
) {
  const [faceHeight, onFaceHeightChange] = useState(0);
  const [controls, setControls] = useState({ total: 0, disclosure: 0 });
  const onContentHeightChange = useCallback(
    (total: number, disclosure = 0) =>
      setControls((previous) =>
        previous.total === total && previous.disclosure === disclosure
          ? previous
          : { total, disclosure },
      ),
    [],
  );
  const updateNodeInternals = useUpdateNodeInternals();
  const face = graphNodeFaceGeometry(node);
  const compact = graphNodeFaceMode(node) === 'compact';
  const portCount = Math.max(node.inputPorts.length, node.outputPorts.length);
  const compactPortHeight = Math.max(
    minimumAudioNodeFaceHeight() + 2,
    audioNodeFrame.headerHeight + (portCount + 1) * 18,
  );
  const collapsedHeight = controls.total - controls.disclosure;
  const baseFaceHeight =
    !compact && node.size
      ? Math.max(face.height, node.size.height - collapsedHeight - 2)
      : face.height;
  const minHeight = compact
    ? Math.max(compactPortHeight, face.height + controls.total + 2)
    : face.height + controls.total + 2;
  const nativeHeight = nativeEditorHeight(node, editor);
  const ports = compact
    ? {
        ...face,
        // Disclosure content may grow the card, but only an editor or explicit resize moves ports.
        height: Math.max(compactPortHeight, node.size?.height ?? nativeHeight),
        portInsetTop: audioNodeFrame.headerHeight,
        portInsetBottom: 12,
      }
    : {
        ...face,
        height: node.size ? baseFaceHeight : faceHeight || face.height,
        firstChannelCenter: channelCenter(
          face.firstChannelCenter,
          node.size ? baseFaceHeight : faceHeight,
          face.height,
        ),
      };
  const signature = [...node.inputPorts, ...node.outputPorts].map((port) => port.id).join('|');
  useEffect(() => {
    void signature;
    void editor.width;
    void ports.height;
    void ports.firstChannelCenter;
    void minHeight;
    updateNodeInternals(node.id);
  }, [
    node.id,
    signature,
    editor.width,
    ports.height,
    ports.firstChannelCenter,
    minHeight,
    updateNodeInternals,
  ]);
  return {
    face,
    faceHeight: baseFaceHeight,
    ports,
    minHeight,
    height: node.size ? node.size.height + controls.disclosure : undefined,
    disclosureHeight: controls.disclosure,
    onFaceHeightChange,
    onContentHeightChange,
  };
}
