import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import type { GraphApplyResponse } from '../core/domain/control/graph';
import type { ProcessorParameterUpdateResponse } from '../core/domain/control/processors';
import { ProblemsTarget } from '../test-support/ProblemsTarget';
import '../test-support/integration';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../core/audio-graph/model';
import { i18n } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import { RoutingWorkspace } from '../features/routing-workspace/RoutingWorkspace';
import { StudioScreen } from './StudioScreen';

vi.mock('../core/audio-graph/AudioGraphCanvas', () => ({
  AudioGraphCanvas: ({
    model,
    onModelChange,
    onNodeParameterChange,
  }: {
    model: AudioGraphModel;
    onModelChange: (model: AudioGraphModel, kind: 'runtime') => void;
    onNodeParameterChange: (nodeId: string, id: string, value: number) => void;
  }) => (
    <section>
      <output data-testid="parameter-value">
        {
          model.nodes
            .find((node) => node.id === 'p')
            ?.appearance.parameterControls?.find((control) => control.id === 'external:42')?.value
        }
      </output>
      <button
        type="button"
        onClick={() =>
          onModelChange({ ...model, edges: [], revision: model.revision + 1 }, 'runtime')
        }
      >
        structural edit
      </button>
      <button
        type="button"
        disabled={
          !model.nodes.some((node) =>
            node.appearance.parameterControls?.some((control) => control.id === 'external:42'),
          )
        }
        onClick={() => onNodeParameterChange('p', 'external:42', 0.2)}
      >
        parameter edit
      </button>
    </section>
  ),
}));

function fixture(onBusyChange?: (busy: boolean) => void, studio = false) {
  const port = new StorybookMockControlPort({
    graph: {
      schemaVersion: 2,
      revision: 1,
      edges: [],
      nodes: [
        {
          id: 'p',
          label: null,
          type: 'provider',
          config: {
            typeId: 'vst3:test',
            parameters: {
              stateRef: { sha256: 'a'.repeat(64), byteLength: 8 },
              parameterValues: { '42': 0.5 },
            },
          },
        },
      ],
    },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
    processors: {
      protocolVersion: 1,
      processors: [
        {
          typeId: 'vst3:test',
          displayName: 'Test',
          format: 'vst3',
          hasEditor: false,
          layouts: [
            {
              layoutId: 'layout-v1:stereo',
              inputs: [
                {
                  id: 'audio',
                  name: 'Main Input',
                  kind: 'main',
                  channels: 2,
                  channelFormat: 'stereo',
                },
              ],
              outputs: [
                {
                  id: 'audio',
                  name: 'Main Output',
                  kind: 'main',
                  channels: 2,
                  channelFormat: 'stereo',
                },
              ],
            },
          ],
          defaultLayoutId: 'layout-v1:stereo',
          vendor: 'Test',
          version: '1',
          parameters: [
            {
              id: 42,
              name: 'Level',
              defaultValue: 0.5,
              canAutomate: true,
              isReadOnly: false,
              stepCount: 0,
            },
          ],
        },
      ],
    },
  });
  const view = render(
    <MixRelayProvider>
      <I18nextProvider i18n={i18n}>
        {studio ? (
          <StudioScreen controlPort={port} />
        ) : (
          <ProblemsTarget>
            {(target) => (
              <RoutingWorkspace
                controlPort={port}
                onBusyChange={onBusyChange}
                problemsTarget={target}
              />
            )}
          </ProblemsTarget>
        )}
      </I18nextProvider>
    </MixRelayProvider>,
  );
  return { port, view };
}

it('retains explicit parameter Undo/Redo intent while graph submissions are in flight', async () => {
  await i18n.changeLanguage('en');
  const { port, view } = fixture();
  const first = Promise.withResolvers<GraphApplyResponse>();
  const second = Promise.withResolvers<GraphApplyResponse>();
  const apply = vi
    .spyOn(port, 'applyGraph')
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);
  const parameter = vi.spyOn(port, 'setProcessorParameter');
  await waitFor(() => expect(screen.getByRole('button', { name: 'parameter edit' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'structural edit' }));
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
  expect(apply.mock.calls[0]?.[0]).not.toHaveProperty('processorParameterChanges');
  fireEvent.click(screen.getByRole('button', { name: 'parameter edit' }));
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
  await act(async () => first.resolve({ protocolVersion: 1, revision: 2, state: 'active' }));
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
  expect(apply.mock.calls[1]?.[0].processorParameterChanges).toEqual([
    { nodeId: 'p', typeId: 'vst3:test', parameterId: 42, value: 0.5 },
  ]);
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true, shiftKey: true });
  await act(async () => second.resolve({ protocolVersion: 1, revision: 3, state: 'active' }));
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(3));
  expect(apply.mock.calls[2]?.[0].processorParameterChanges).toEqual([
    { nodeId: 'p', typeId: 'vst3:test', parameterId: 42, value: 0.2 },
  ]);
  expect(parameter).not.toHaveBeenCalled();
  view.unmount();
});

it('awaits direct parameter acknowledgement before a structural rebuild, without replaying stale fields', async () => {
  await i18n.changeLanguage('en');
  const { port, view } = fixture();
  const pending = Promise.withResolvers<ProcessorParameterUpdateResponse>();
  const parameter = vi.spyOn(port, 'setProcessorParameter').mockReturnValueOnce(pending.promise);
  const apply = vi.spyOn(port, 'applyGraph');
  await waitFor(() => expect(screen.getByRole('button', { name: 'parameter edit' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'parameter edit' }));
  expect(parameter).toHaveBeenCalledWith('p', 42, 0.2, 1);
  fireEvent.click(screen.getByRole('button', { name: 'structural edit' }));
  await act(async () => Promise.resolve());
  expect(apply).not.toHaveBeenCalled();
  await act(async () =>
    pending.resolve({ protocolVersion: 1, nodeId: 'p', parameterId: 42, value: 0.2 }),
  );
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
  expect(apply.mock.calls[0]?.[0]).not.toHaveProperty('processorParameterChanges');
  view.unmount();
});

it('shows native parameter rejection, keeps the draft and blocks workspace mutations until Retry succeeds', async () => {
  await i18n.changeLanguage('en');
  const busy = vi.fn();
  const { port, view } = fixture(busy);
  const parameter = vi
    .spyOn(port, 'setProcessorParameter')
    .mockRejectedValueOnce(new Error('offline'));
  const apply = vi.spyOn(port, 'applyGraph');
  await waitFor(() => expect(screen.getByRole('button', { name: 'parameter edit' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'parameter edit' }));
  await screen.findByText('The audio control request failed.');
  expect(screen.getByTestId('parameter-value')).toHaveTextContent('0.2');
  expect(busy).toHaveBeenLastCalledWith(true);
  expect(view.container.querySelector('section[data-state]')).toHaveAttribute(
    'data-state',
    'blocked',
  );
  expect(apply).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(busy).toHaveBeenLastCalledWith(false));
  expect(parameter).toHaveBeenLastCalledWith('p', 42, 0.2, 1);
  expect(parameter).toHaveBeenCalledTimes(2);
  expect(screen.queryByText('The audio control request failed.')).not.toBeInTheDocument();
  expect(screen.getByTestId('parameter-value')).toHaveTextContent('0.2');
  view.unmount();
});

it('keeps failed intent after a failed reload and clears it only after an authoritative reload succeeds', async () => {
  await i18n.changeLanguage('en');
  const busy = vi.fn();
  const { port, view } = fixture(busy);
  const parameter = vi
    .spyOn(port, 'setProcessorParameter')
    .mockRejectedValueOnce(new Error('offline'));
  await waitFor(() => expect(screen.getByRole('button', { name: 'parameter edit' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'parameter edit' }));
  const parameterRetry = await screen.findByRole('button', { name: 'Retry' });
  const getGraph = vi.spyOn(port, 'getGraph').mockRejectedValue(new Error('reload offline'));
  fireEvent.click(screen.getByRole('button', { name: 'Discard edits and reload engine' }));
  await waitFor(() => expect(getGraph).toHaveBeenCalledTimes(2));
  expect(parameterRetry).toBeInTheDocument();
  expect(screen.getByTestId('parameter-value')).toHaveTextContent('0.2');
  expect(busy).toHaveBeenLastCalledWith(true);
  getGraph.mockRestore();
  const reload = screen.getAllByRole('button', { name: 'Discard edits and reload engine' })[0];
  if (!reload) throw new Error('Missing reload action');
  fireEvent.click(reload);
  await waitFor(() => expect(screen.getByTestId('parameter-value')).toHaveTextContent('0.5'));
  await waitFor(() => expect(busy).toHaveBeenLastCalledWith(false));
  expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
  expect(parameter).toHaveBeenCalledTimes(1);
  view.unmount();
});

it('refuses structural graph application while a native parameter failure remains unresolved', async () => {
  await i18n.changeLanguage('en');
  const { port, view } = fixture();
  vi.spyOn(port, 'setProcessorParameter').mockRejectedValueOnce(new Error('offline'));
  const apply = vi.spyOn(port, 'applyGraph');
  const getGraph = vi.spyOn(port, 'getGraph');
  await waitFor(() => expect(screen.getByRole('button', { name: 'parameter edit' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'parameter edit' }));
  await screen.findByRole('button', { name: 'Retry' });
  getGraph.mockClear();
  fireEvent.click(screen.getByRole('button', { name: 'structural edit' }));
  await waitFor(() => expect(getGraph).toHaveBeenCalledTimes(1));
  expect(apply).not.toHaveBeenCalled();
  expect(screen.getByTestId('parameter-value')).toHaveTextContent('0.2');
  expect(view.container.querySelector('section[data-state]')).toHaveAttribute(
    'data-state',
    'blocked',
  );
  view.unmount();
});

it('keeps actual Studio catalog mutation controls fenced while native parameter intent is unresolved', async () => {
  await i18n.changeLanguage('en');
  const { port, view } = fixture(undefined, true);
  vi.spyOn(port, 'setProcessorParameter').mockRejectedValueOnce(new Error('offline'));
  const mutate = vi.spyOn(port, 'mutateGraphCatalog');
  const create = await screen.findByRole('button', { name: 'New graph' });
  await waitFor(() => expect(create).toBeEnabled());
  await waitFor(() => expect(screen.getByRole('button', { name: 'parameter edit' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'parameter edit' }));
  // The failure is counted on the diagnostics button, not shown over the workspace.
  const diagnostics = await screen.findByRole('button', { name: 'Diagnostics · 1' });
  expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
  expect(create).toBeEnabled();
  fireEvent.click(create);
  const dialog = screen.getByRole('dialog', { name: 'New graph' });
  const submit = within(dialog).getByRole('button', { name: 'Create' });
  expect(submit).toBeDisabled();
  fireEvent.click(submit);
  const form = submit.closest('form');
  if (!form) throw new Error('Missing create form');
  fireEvent.submit(form);
  expect(mutate).not.toHaveBeenCalled();
  expect(screen.getByTestId('parameter-value')).toHaveTextContent('0.2');
  const cancel = within(dialog).getByRole('button', { name: 'Cancel' });
  expect(cancel).toBeEnabled();
  fireEvent.click(cancel);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  fireEvent.keyDown(window, { key: 'n', ctrlKey: true, shiftKey: true });
  expect(screen.getByRole('dialog', { name: 'New graph' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
  expect(mutate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  fireEvent.click(diagnostics);
  const report = screen.getByRole('dialog', { name: 'Diagnostics' });
  fireEvent.click(await within(report).findByRole('button', { name: 'Retry' }));
  await waitFor(() =>
    expect(within(report).queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument(),
  );
  fireEvent.click(within(report).getByRole('button', { name: 'Close' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Diagnostics' })).toBeVisible());
  fireEvent.click(create);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Create' })).toBeEnabled());
  view.unmount();
});
