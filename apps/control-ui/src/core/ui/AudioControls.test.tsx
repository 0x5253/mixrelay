import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard } from './AudioNodeCard';
import { LevelControl } from './LevelControl';
import { Meter } from './Meter';
import { Panel } from './Panel';
import { ToggleField } from './ToggleField';

it('selects a native device rate without changing its observed value optimistically', () => {
  const onChange = vi.fn();
  render(
    <MixRelayProvider>
      <AudioNodeCard
        category="Device"
        icon="IO"
        title="LAB"
        inputCount={2}
        outputCount={2}
        onSelect={vi.fn()}
        optionsLabel="Options"
        onEndpointChange={onChange}
        endpointControl={{
          displayName: 'LAB',
          nameLabel: 'Device name',
          renderChannels: 2,
          captureChannels: 2,
          observedRate: { label: 'Device sample rate', value: 48000 },
          sampleRateOptions: [44100, 48000],
        }}
      />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Options') }));
  const nameField = screen.getByRole('textbox', { name: 'Device name' });
  const channels = screen.getByRole('button', { name: /Channels$/ });
  const rateField = screen.getByRole('button', { name: /Device sample rate$/ });
  expect(
    nameField.compareDocumentPosition(channels) & Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
  expect(
    channels.compareDocumentPosition(rateField) & Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: /Device sample rate$/ }));
  fireEvent.click(screen.getByRole('option', { name: '44.1 kHz' }));
  expect(onChange).toHaveBeenCalledWith({
    displayName: 'LAB',
    renderChannels: 2,
    captureChannels: 2,
    sampleRate: 44100,
  });
  expect(screen.getByRole('button', { name: /Device sample rate$/ })).toHaveTextContent('48 kHz');
});

describe('Native endpoint controls', () => {
  it('renders fixed native endpoint settings read-only', () => {
    const onChange = vi.fn();
    render(
      <MixRelayProvider>
        <AudioNodeCard
          category="Device"
          icon="IO"
          title="LAB"
          inputCount={1}
          outputCount={2}
          onSelect={vi.fn()}
          optionsLabel="Options"
          onEndpointChange={onChange}
          endpointControl={{
            displayName: 'LAB',
            nameLabel: 'Device name',
            renderChannels: 2,
            captureChannels: 1,
            canRename: false,
            observedRate: { label: 'Device sample rate', value: 44100 },
            allowedRenderChannels: [2],
            allowedCaptureChannels: [1],
          }}
        />
      </MixRelayProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Options') }));
    expect(screen.getByLabelText('Device name')).toHaveAttribute('readonly');
    expect(screen.getByRole('button', { name: /Channels$/ })).toBeDisabled();
    expect(screen.getByRole('button', { name: /Channels$/ })).toHaveTextContent('Mixed');
    expect(screen.getByLabelText('Device sample rate')).toHaveValue('44.1 kHz');
    expect(screen.getByLabelText('Device sample rate')).toHaveAttribute('readonly');
    fireEvent.blur(screen.getByLabelText('Device name'));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('Core audio controls', () => {
  it('forwards node, toggle, and level interactions', () => {
    const onSelect = vi.fn();
    const onToggle = vi.fn();
    const onLevel = vi.fn();

    render(
      <MixRelayProvider>
        <div>
          <AudioNodeCard
            category="Source"
            detail="Test source"
            icon="T"
            inputCount={1}
            onSelect={onSelect}
            outputCount={2}
            title="Reusable node"
          />
          <AudioNodeCard
            bypassed
            category="Output"
            detail="OBS input"
            icon="O"
            inputCount={2}
            onSelect={vi.fn()}
            outputCount={0}
            selected
            showPorts={false}
            status="Ready"
            statusTone="positive"
            title="Virtual output"
          />
          <ToggleField checked={false} label="Enabled" onChange={onToggle} />
          <LevelControl label="Level" max={10} min={0} onChange={onLevel} suffix="%" value={5} />
          <Meter label="Test" level={42} />
          <Panel title="Panel title">Panel content</Panel>
        </div>
      </MixRelayProvider>,
    );

    const node = screen.getByRole('button', { name: 'Reusable node, Source, Test source' });
    fireEvent.click(node);
    expect(onSelect).toHaveBeenCalledOnce();
    expect(within(node).getByText('Reusable node')).toBeInTheDocument();
    expect(screen.getByLabelText('Ready')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('switch', { name: 'Enabled' }));
    expect(onToggle).toHaveBeenCalledWith(true);

    fireEvent.keyDown(screen.getByRole('slider'), { key: 'ArrowRight' });
    expect(onLevel).toHaveBeenCalledWith(6);
    expect(screen.getByTitle('Test: 42%')).toBeInTheDocument();
  });

  it('offers retry only for a failed native processor editor', () => {
    const onEditorToggle = vi.fn();
    render(
      <MixRelayProvider>
        <AudioNodeCard
          category="Processor"
          icon="FX"
          inputCount={2}
          onProcessorEditorRetry={onEditorToggle}
          onSelect={vi.fn()}
          optionsLabel="Options"
          outputCount={2}
          processorEditor={{
            actionLabel: 'Retry',
            state: 'failed',
          }}
          title="Compressor"
        />
      </MixRelayProvider>,
    );

    expect(
      screen.queryByRole('button', { name: (name) => name.endsWith('Options') }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText('Plug-in editor')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Compressor: Retry' }));
    expect(onEditorToggle).toHaveBeenCalledOnce();
  });
});

it('exposes a read-only meter value without adding a keyboard stop or live announcements', () => {
  const view = render(<Meter label="Input level" level={140} />);
  const meter = screen.getByRole('meter', { name: 'Input level' });
  expect(meter).toHaveAttribute('aria-valuenow', '100');
  expect(meter).not.toHaveAttribute('tabindex');
  expect(meter).not.toHaveAttribute('aria-live');
  view.rerender(<Meter label="Input level" level={-20} />);
  expect(meter).toHaveAttribute('aria-valuenow', '0');
});
