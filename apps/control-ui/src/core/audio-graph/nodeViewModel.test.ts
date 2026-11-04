import { expect, it } from 'vitest';
import type { ProcessorEditorStatus } from '../domain/control/processors';
import type { ProcessorEditorLabels } from './audioGraphCanvasTypes';
import type { AudioGraphNode } from './model';
import { audioFlowNodeWidth, nodeProcessorEditors, processorEditorFace } from './nodeViewModel';

const labels: ProcessorEditorLabels = {
  open: 'Open editor',
  retry: 'Retry editor',
  failure: {
    no_editor: 'No editor',
    display_unavailable: 'No display',
    host_window_failed: 'Window failed',
    plugin_rejected: 'Rejected',
    worker_unavailable: 'Worker unavailable',
  },
};

function status(overrides: Partial<ProcessorEditorStatus>): ProcessorEditorStatus {
  return {
    failure: null,
    height: null,
    nodeId: 'fx',
    protocolVersion: 1,
    resizable: false,
    state: 'closed',
    width: null,
    ...overrides,
  };
}

it('shares processor editors only with both a retry owner and labels', () => {
  const statuses = new Map<string, ProcessorEditorStatus>();
  const retry = () => {};
  expect(nodeProcessorEditors(undefined, labels, statuses)).toBeNull();
  expect(nodeProcessorEditors(retry, undefined, statuses)).toBeNull();
  expect(nodeProcessorEditors(retry, labels, statuses)).toEqual({
    labels,
    onRetry: retry,
    statuses,
  });
});

it('shows an editor action only for an editor-capable node of a shared editor owner', () => {
  const editors = nodeProcessorEditors(() => {}, labels, new Map());
  expect(processorEditorFace('fx', false, editors)).toBeUndefined();
  expect(processorEditorFace('fx', true, null)).toBeUndefined();
  expect(processorEditorFace('fx', true, editors)).toEqual({
    actionLabel: 'Open editor',
    failureLabel: undefined,
    state: 'closed',
  });
});

it('offers a retry with the reported failure once the editor is not closed', () => {
  const statuses = new Map([
    ['fx', status({ state: 'failed', failure: 'plugin_rejected' })],
    ['open', status({ nodeId: 'open', state: 'open' })],
  ]);
  const editors = nodeProcessorEditors(() => {}, labels, statuses);
  expect(processorEditorFace('fx', true, editors)).toEqual({
    actionLabel: 'Retry editor',
    failureLabel: 'Rejected',
    state: 'failed',
  });
  expect(processorEditorFace('open', true, editors)?.failureLabel).toBeUndefined();
});

it('sizes an unsized node to its open editor within card limits, and a stored size wins', () => {
  const node = {
    id: 'fx',
    position: { x: 0, y: 0 },
    inputPorts: [],
    outputPorts: [],
    appearance: { category: 'FX', icon: 'FX', title: 'FX' },
  } as AudioGraphNode;
  expect(audioFlowNodeWidth(node, null)).toBeUndefined();
  expect(audioFlowNodeWidth(node, 100)).toBe(240);
  expect(audioFlowNodeWidth(node, 800)).toBe(800);
  expect(audioFlowNodeWidth(node, 4000)).toBe(1600);
  expect(audioFlowNodeWidth({ ...node, size: { width: 500, height: 300 } }, 800)).toBe(500);
});
