import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { ProcessorEditorStatus } from '../../core/domain/control/processors';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { ProcessorEditorAlert } from './ProcessorControlAlerts';
import { useProcessorEditors } from './useProcessorEditors';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe('processor editor recovery on the node', () => {
  it('shows a rejected automatic open and keeps retry explicit', async () => {
    await i18n.changeLanguage('en');
    const open: ProcessorEditorStatus = {
      protocolVersion: 1,
      nodeId: 'effect',
      state: 'open',
      failure: null,
      height: null,
      width: null,
      resizable: false,
    };
    const pending = Promise.withResolvers<ProcessorEditorStatus>();
    const source = {
      getProcessorEditor: vi.fn().mockResolvedValue({ ...open, state: 'closed' }),
      setProcessorEditor: vi
        .fn()
        .mockRejectedValueOnce(new Error('rejected'))
        .mockReturnValue(pending.promise),
    };
    const graph: AudioGraphModel = {
      revision: 1,
      edges: [],
      nodes: [
        {
          id: 'effect',
          inputPorts: [],
          outputPorts: [],
          position: { x: 0, y: 0 },
          appearance: {
            title: 'My effect',
            category: 'Processor',
            icon: 'effect',
            hasProcessorEditor: true,
          },
          runtime: { type: 'provider', config: { typeId: 'au:fixture' } },
        },
      ],
    };
    function Panel() {
      const editors = useProcessorEditors(source, graph);
      return (
        <>
          <output data-testid="native-state">{editors.statuses.get('effect')?.state}</output>
          {[...editors.issues.values()].map((issue) => (
            <ProcessorEditorAlert
              key={issue.nodeId}
              issue={issue}
              onRetry={() => editors.retry(issue.nodeId)}
            />
          ))}
        </>
      );
    }
    render(
      <I18nextProvider i18n={i18n}>
        <MixRelayProvider>
          <Panel />
        </MixRelayProvider>
      </I18nextProvider>,
    );
    expect(await screen.findByRole('alert')).toHaveTextContent('My effect');
    expect(screen.getByTestId('native-state')).toHaveTextContent('closed');
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
    await act(async () => pending.resolve(open));
    expect(source.setProcessorEditor.mock.calls).toEqual([
      ['effect', true, 1],
      ['effect', true, 1],
    ]);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.getByTestId('native-state')).toHaveTextContent('open');
  });
});
