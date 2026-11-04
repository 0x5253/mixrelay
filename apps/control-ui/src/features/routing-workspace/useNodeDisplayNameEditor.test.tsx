import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { GraphLayoutController } from '../../core/application/workspace/GraphLayoutController';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { unexpected } from '../../test-support/portDefaults';
import { routingLayoutProjection } from './runtimeGraphProjection';
import type { useGraphLayout } from './useGraphLayout';
import { presentNodeDisplayNames, useNodeDisplayNameEditor } from './useNodeDisplayNameEditor';

const model: AudioGraphModel = {
  revision: 7,
  edges: [],
  nodes: ['video', 'meeting'].map((id) => ({
    id,
    appearance: { title: 'Firefox', category: 'Application', icon: 'APP' },
    position: { x: 0, y: 0 },
    inputPorts: [],
    outputPorts: [],
    runtime: { type: 'source.native.application', config: { processId: 12 } },
  })),
};
function fixture() {
  const controller = new GraphLayoutController(
    {
      getGraphCatalog: unexpected('getGraphCatalog'),
      getGraphLayout: unexpected('getGraphLayout'),
      setGraphLayout: unexpected('setGraphLayout'),
    },
    'graph',
    timerScheduler,
    monotonicClock,
    routingLayoutProjection,
  );
  return {
    controller,
    loaded: true,
    loading: false,
    layout: { positions: {}, nodeLabels: { video: 'Video' } },
    updateNodeLabel: vi.fn(),
  } as unknown as ReturnType<typeof useGraphLayout>;
}
function Harness({
  layout,
  value = model,
  disabled = false,
}: {
  layout: ReturnType<typeof useGraphLayout>;
  value?: AudioGraphModel | null;
  disabled?: boolean;
}) {
  const editor = useNodeDisplayNameEditor(value, layout, disabled);
  return (
    <>
      <button type="button" onClick={() => editor.request?.('video')}>
        Rename
      </button>
      <button type="button" onClick={() => editor.request?.('missing')}>
        Missing
      </button>
      {editor.dialog}
    </>
  );
}

describe('graph instance display names', () => {
  it('labels each UI instance without changing source names, config, identity or audio revision', () => {
    expect(presentNodeDisplayNames(model)).toBe(model);
    const named = presentNodeDisplayNames(model, { video: 'Video', meeting: 'Meeting' });
    expect(named.nodes.map((node) => node.displayName)).toEqual(['Video', 'Meeting']);
    expect(named.nodes.map((node) => node.appearance.title)).toEqual(['Firefox', 'Firefox']);
    expect(named.nodes[0]?.runtime).toBe(model.nodes[0]?.runtime);
    expect(named.revision).toBe(7);
    expect(presentNodeDisplayNames(named, { video: 'Video', meeting: 'Meeting' })).toBe(named);
    expect(presentNodeDisplayNames(named).nodes.map((node) => node.displayName)).toEqual([
      undefined,
      undefined,
    ]);
  });
  it('submits only a presentation label, clears the original name override, and cancels without writing', () => {
    const layout = fixture();
    render(
      <MixRelayProvider>
        <Harness layout={layout} />
      </MixRelayProvider>,
    );
    const open = () => fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    open();
    const input = () => screen.getByRole('textbox', { name: i18n.t('routing.nodeName.label') });
    expect(input()).toHaveValue('Video');
    fireEvent.change(input(), { target: { value: '  Meeting browser  ' } });
    fireEvent.click(screen.getByRole('button', { name: i18n.t('routing.nodeName.save') }));
    expect(layout.updateNodeLabel).toHaveBeenLastCalledWith('video', 'Meeting browser');
    open();
    fireEvent.change(input(), { target: { value: 'Firefox' } });
    fireEvent.click(screen.getByRole('button', { name: i18n.t('routing.nodeName.save') }));
    expect(layout.updateNodeLabel).toHaveBeenLastCalledWith('video', '');
    open();
    fireEvent.keyDown(input(), { key: 'Escape' });
    expect(layout.updateNodeLabel).toHaveBeenCalledTimes(2);
  });
  it('uses the source name without an alias and cannot carry an editor into another graph owner', () => {
    const layout = fixture();
    layout.layout = { positions: {} };
    const view = render(
      <MixRelayProvider>
        <Harness layout={layout} />
      </MixRelayProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    expect(screen.getByRole('textbox')).toHaveValue('');
    expect(screen.getByRole('textbox')).toHaveAttribute('placeholder', 'Firefox');
    expect(screen.getByRole('textbox')).not.toHaveAccessibleDescription();
    view.rerender(
      <MixRelayProvider>
        <Harness layout={fixture()} />
      </MixRelayProvider>,
    );
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
  it('does not edit a deleted node or a null model, a loading/unsupported/disabled workspace', () => {
    const layout = fixture();
    const view = render(
      <MixRelayProvider>
        <Harness layout={layout} />
      </MixRelayProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Missing' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    view.rerender(
      <MixRelayProvider>
        <Harness layout={layout} value={null} />
      </MixRelayProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    for (const updated of [
      { ...layout, loaded: false },
      { ...layout, loading: true },
      {
        ...layout,
        // Without a graph there is no layout to edit.
        controller: new GraphLayoutController(
          {
            getGraphCatalog: unexpected('getGraphCatalog'),
            getGraphLayout: unexpected('getGraphLayout'),
            setGraphLayout: unexpected('setGraphLayout'),
          },
          undefined,
          timerScheduler,
          monotonicClock,
          routingLayoutProjection,
        ),
      },
    ]) {
      view.rerender(
        <MixRelayProvider>
          <Harness layout={updated} />
        </MixRelayProvider>,
      );
      fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    }
    view.rerender(
      <MixRelayProvider>
        <Harness layout={layout} disabled />
      </MixRelayProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});

it('treats opaque instance IDs as own metadata keys, including prototype names', () => {
  const opaque = {
    ...model,
    nodes: model.nodes.map((node, index) => ({
      ...node,
      id: index === 0 ? '__proto__' : 'constructor',
    })),
  };
  expect(presentNodeDisplayNames(opaque, {})).toBe(opaque);
  const labels = Object.fromEntries([
    ['__proto__', 'Video'],
    ['constructor', 'Meeting'],
  ]);
  expect(presentNodeDisplayNames(opaque, labels).nodes.map((node) => node.displayName)).toEqual([
    'Video',
    'Meeting',
  ]);
});
