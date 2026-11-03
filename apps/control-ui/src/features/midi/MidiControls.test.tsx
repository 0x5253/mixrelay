import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useContext, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { MidiAssignment, MidiBinding, MidiStatus } from '../../core/domain/midi';
import { I18nProvider, i18n } from '../../core/i18n';
import { MidiGraphContext, MidiNodeContext, MidiPanelContext } from '../../core/midi/MidiContext';
import type { MidiPort } from '../../core/ports/MidiPort';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioNodeCard } from '../../core/ui/AudioNodeCard';
import { NodeMuteButton, NodePowerButton } from '../../core/ui/NodeSignalControls';
import { TransportButton } from '../../core/ui/TransportButton';
import { advance, find, finishFakeTimerTest } from '../../test-support/fakeTimerQueries';
import { MidiAccess, MidiSettingsButton } from './MidiAccess';
import { MidiBindingOptions } from './MidiBindingOptions';
import { MidiBindingsList } from './MidiBindingsList';
import { midiBinding, midiFixture, midiPortFixture } from './midiFixtures';

let user: ReturnType<typeof userEvent.setup>;

beforeEach(async () => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  await i18n.changeLanguage('en');
});
afterEach(() => finishFakeTimerTest());
function view(children: ReactNode) {
  return render(
    <MixRelayProvider>
      <I18nProvider>{children}</I18nProvider>
    </MixRelayProvider>,
  );
}

const keyboard = (keys: string) => user.keyboard(keys);
const click = (name: string | RegExp) => user.click(screen.getByRole('button', { name }));
const audio = vi.fn();
function controls() {
  return (
    <MidiGraphContext.Provider value={{ graphId: 'main', graphName: 'Programme' }}>
      <MidiNodeContext.Provider value={{ nodeId: 'gain', nodeName: 'Microphone' }}>
        <AudioNodeCard
          title="Microphone"
          category="Gain"
          icon="gain"
          inputCount={2}
          outputCount={2}
          channelCount={2}
          gainControl={{ label: 'Volume', valueDb: 0 }}
          onGainChange={audio}
          headerActions={
            <NodePowerButton enabled disabled={false} label="Power" onChange={audio} />
          }
          muteControl={
            <NodeMuteButton muted={false} disabled={false} label="Mute" onChange={audio} />
          }
        />
        {(['record', 'pause', 'stop', 'resume'] as const).map((icon) => (
          <TransportButton key={icon} icon={icon} label={icon} disabled={false} onClick={audio} />
        ))}
      </MidiNodeContext.Provider>
    </MidiGraphContext.Provider>
  );
}
function PanelOutlet() {
  return useContext(MidiPanelContext);
}
function access(port: MidiPort | null) {
  return view(
    <MidiAccess port={port}>
      <MidiSettingsButton />
      <PanelOutlet />
      {controls()}
    </MidiAccess>,
  );
}
it('one MIDI toggle enters mapping, replaces audio controls, selects with keyboard and exits to its trigger', async () => {
  const port = midiPortFixture();
  const command = vi.spyOn(port, 'command');
  audio.mockClear();
  access(port);
  const toggle = await find('button', { name: 'MIDI map mode' });
  expect(toggle).toHaveAttribute('aria-pressed', 'false');
  await click('MIDI map mode');
  expect(toggle).toHaveAttribute('aria-pressed', 'true');
  const volume = await find('button', { name: 'Map Microphone: Volume' });
  expect(volume).toHaveTextContent('CC 7');
  volume.focus();
  await keyboard(' ');
  expect(command).toHaveBeenCalledWith('midi_learn', {
    assignment: {
      target: midiBinding.target,
      action: 'volume',
      graphName: 'Programme',
      nodeName: 'Microphone',
    },
  });
  expect(volume).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByText(/move a MIDI control to assign or replace/)).toBeVisible();
  await click('Map Microphone: Power');
  await click('Map Microphone: Mute');
  await click('Map Microphone: Pause');
  await click('Map Microphone: Stop');
  const records = screen.getAllByRole('button', { name: 'Map Microphone: Record / resume' });
  await user.click(records[0] as HTMLElement);
  expect(audio).not.toHaveBeenCalled();
  await keyboard('{Escape}');
  expect(toggle).toHaveAttribute('aria-pressed', 'false');
  expect(toggle).toHaveFocus();
  expect(screen.queryByRole('complementary')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'MIDI controls' })).not.toBeInTheDocument();
  await click('MIDI map mode');
  await click('Finish mapping');
  expect(toggle).toHaveFocus();
});
it.each(['MacIntel', 'Linux x86_64'])(
  'uses the platform shortcut on %s and ignores wrong modifiers and repeated editing keys',
  async (platform) => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
    const port = midiPortFixture();
    const command = vi.spyOn(port, 'command');
    access(port);
    await advance();
    await keyboard(platform === 'MacIntel' ? '{Control>}m{/Control}' : '{Meta>}m{/Meta}');
    expect(command).not.toHaveBeenCalledWith('midi_map', expect.anything());
    await keyboard(platform === 'MacIntel' ? '{Meta>}m{/Meta}' : '{Control>}m{/Control}');
    expect(await find('button', { name: 'MIDI map mode' })).toHaveAttribute('aria-pressed', 'true');
    await keyboard(platform === 'MacIntel' ? '{Meta>}m{/Meta}' : '{Control>}m{/Control}');
    expect(screen.getByRole('button', { name: 'MIDI map mode' })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
  },
);
it('Delete on a mapped target removes one complete mapping identity and ordinary keys do not remove it', async () => {
  const port = midiPortFixture();
  const command = vi.spyOn(port, 'command');
  access(port);
  await click('MIDI map mode');
  const volume = await find('button', { name: 'Map Microphone: Volume' });
  volume.focus();
  await keyboard('{ArrowLeft}{Delete}');
  expect(command).toHaveBeenCalledWith('midi_remove', { binding: midiBinding });
  expect(screen.getByRole('button', { name: 'MIDI map mode' })).toHaveFocus();
});
it('keeps several mappings on one control distinct and shows remembered names without internal IDs', async () => {
  const second = {
    ...midiBinding,
    target: { graphId: 'deleted-secret', nodeId: 'internal-secret' },
    nodeName: 'Programme output',
  };
  const onRemove = vi.fn();
  const onSelect = vi.fn();
  view(
    <MidiBindingsList
      status={{ ...midiFixture, targets: [], devices: [], bindings: [midiBinding, second] }}
      busy={false}
      selected={null}
      onSelect={onSelect}
      onChange={vi.fn()}
      onRemove={onRemove}
    />,
  );
  expect(screen.getAllByText('Target missing')).toHaveLength(2);
  expect(screen.getAllByText('Device disconnected')).toHaveLength(2);
  expect(screen.queryByText(/internal-secret|deleted-secret/)).not.toBeInTheDocument();
  expect(
    screen.getByRole('button', { name: 'Select Programme / Programme output · Volume' }),
  ).toHaveTextContent('Programme / Programme output · Volume');
  const row = screen.getByRole('button', { name: 'Select Programme / Programme output · Volume' });
  row.focus();
  await keyboard('{Enter}{Backspace}');
  expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ target: second.target }));
  expect(onRemove).toHaveBeenCalledWith(second);
  await click('Remove MIDI control for Programme / Microphone');
  expect(onRemove).toHaveBeenLastCalledWith(midiBinding);
});
function ListOptions({ binding = midiBinding }: { binding?: MidiBinding }) {
  const [selected, setSelected] = useState<MidiAssignment | null>(null);
  return (
    <MidiBindingsList
      status={{ ...midiFixture, bindings: [binding] }}
      busy={false}
      selected={selected}
      onSelect={setSelected}
      onChange={changed}
      onRemove={vi.fn()}
    />
  );
}
const changed = vi.fn();
it.each([
  ['Minimum', '{End}', { minDb: 12 }],
  ['Maximum', '{Home}', { maxDb: -60 }],
  ['Volume mode', '{End}', { mode: 'offset64' }],
] as const)(
  'edits %s only on its selected mapping through the Core select',
  async (label, key, update) => {
    view(<ListOptions />);
    await click('Select Programme / Microphone · Volume');
    screen.getByRole('button', { name: new RegExp(label) }).focus();
    await keyboard(` ${key}{Enter}`);
    expect(changed).toHaveBeenLastCalledWith(midiBinding, { ...midiBinding, ...update });
  },
);
it('relative steps and recorder choices update only their selected mapping', async () => {
  const relative = { ...midiBinding, mode: 'twos_complement' as const };
  const first = view(<ListOptions binding={relative} />);
  await click('Select Programme / Microphone · Volume');
  screen.getByRole('button', { name: /Step/ }).focus();
  await keyboard(' {End}{Enter}');
  expect(changed).toHaveBeenLastCalledWith(relative, { ...relative, stepDb: 3 });
  first.unmount();
  const record = { ...midiBinding, action: 'record' as const };
  view(<ListOptions binding={record} />);
  await click('Select Programme / Microphone · Record / resume');
  screen.getByRole('button', { name: /Recorder action/ }).focus();
  await keyboard(' {End}{Enter}');
  expect(changed).toHaveBeenLastCalledWith(record, { ...record, action: 'stop' });
});
it('a non-volume toggle has no volume options and unavailable observations do not mean deleted targets', () => {
  view(
    <>
      <MidiBindingOptions
        binding={{ ...midiBinding, action: 'mute' }}
        busy={false}
        onChange={vi.fn()}
      />
      <MidiBindingsList
        status={{ ...midiFixture, targetsAvailable: false, targets: [] }}
        busy={false}
        selected={null}
        onSelect={vi.fn()}
        onChange={vi.fn()}
        onRemove={vi.fn()}
      />
    </>,
  );
  expect(screen.getByText('Targets unavailable')).toBeVisible();
  expect(screen.queryByText('Target missing')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Volume mode/ })).not.toBeInTheDocument();
});
it('shows empty and MIDI unavailable states with the actual reason', () => {
  view(
    <MidiBindingsList
      status={{ ...midiFixture, bindings: [], unavailable: 'No sequencer access' }}
      busy={false}
      selected={null}
      onSelect={vi.fn()}
      onChange={vi.fn()}
      onRemove={vi.fn()}
    />,
  );
  expect(screen.getByText('No sequencer access')).toBeVisible();
  expect(screen.getByText(/No MIDI mappings/)).toBeVisible();
});
it('native failures remain visible, failed mode changes do not claim success, and browsers expose no MIDI toggle', async () => {
  const current = access({ command: vi.fn().mockRejectedValue(new Error('offline')) });
  await click('MIDI map mode');
  expect(await find('alert')).toHaveTextContent('Could not update MIDI controls');
  expect(screen.getByRole('button', { name: 'MIDI map mode' })).toHaveAttribute(
    'aria-pressed',
    'false',
  );
  current.unmount();
  access(null);
  expect(screen.queryByRole('button', { name: 'MIDI map mode' })).not.toBeInTheDocument();
});

it('the panel selects and highlights a mapping in mode, edits only that row, and removes it with complete identity', async () => {
  const port = midiPortFixture();
  const command = vi.spyOn(port, 'command');
  access(port);
  await click('MIDI map mode');
  await click('Select Programme / Microphone · Volume');
  screen.getByRole('button', { name: /Volume mode/ }).focus();
  await keyboard(' {End}{Enter}');
  expect(command).toHaveBeenCalledWith('midi_save', {
    identity: midiBinding,
    binding: { ...midiBinding, mode: 'offset64' },
  });
  await click('Remove MIDI control for Programme / Microphone');
  expect(command).toHaveBeenCalledWith('midi_remove', { binding: midiBinding });
  await click('Select Programme / Microphone · Volume');
  expect(command).toHaveBeenCalledWith('midi_learn', expect.anything());
});
it('loading and unsuccessful selection, removal or exit preserve the current map state', async () => {
  const pending = Promise.withResolvers<import('../../core/domain/midi').MidiStatus>();
  const command = vi
    .fn<MidiPort['command']>()
    .mockReturnValueOnce(pending.promise)
    .mockRejectedValue(new Error('offline'));
  access({ command });
  expect(screen.queryByRole('complementary')).not.toBeInTheDocument();
  pending.resolve({ ...midiFixture, mapMode: true });
  await advance();
  await click('Map Microphone: Volume');
  expect(screen.getByRole('button', { name: 'Map Microphone: Volume' })).toHaveAttribute(
    'aria-pressed',
    'false',
  );
  await click('Remove MIDI control for Programme / Microphone');
  await click('Finish mapping');
  expect(screen.getByRole('button', { name: 'MIDI map mode' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  expect(await find('alert')).toHaveTextContent('Could not update MIDI controls');
});

it('unavailable mapping scopes never expose a false volume target and MIDI key repeat/composition does not toggle modes', async () => {
  const { MidiContext } = await import('../../core/midi/MidiContext');
  const context = {
    active: true,
    status: midiFixture,
    selected: null,
    toggle: vi.fn(),
    assign: vi.fn(),
    remove: vi.fn(),
  };
  const incomplete = view(
    <MidiContext.Provider value={context}>
      <AudioNodeCard
        title="No gain"
        category="Gain"
        icon="gain"
        inputCount={0}
        outputCount={0}
        channelCount={0}
        gainControl={{ label: 'Volume', valueDb: 0 }}
        onGainChange={vi.fn()}
      />
    </MidiContext.Provider>,
  );
  expect(screen.queryByRole('button', { name: /Map .*Volume/ })).not.toBeInTheDocument();
  incomplete.unmount();
  const absent = view(
    <MidiContext.Provider value={context}>
      <MidiGraphContext.Provider value={{ graphId: 'main', graphName: 'Programme' }}>
        <MidiNodeContext.Provider value={{ nodeId: 'gain', nodeName: 'Microphone' }}>
          <AudioNodeCard
            parameterControls={[
              {
                id: 'test',
                label: 'Parameter',
                minimum: 0,
                maximum: 1,
                step: 0.1,
                value: 0,
                suffix: '',
              },
            ]}
            onParameterChange={vi.fn()}
            title="No level"
            category="Gain"
            icon="gain"
            inputCount={0}
            outputCount={0}
            channelCount={0}
          />
          <AudioNodeCard
            parameterControls={[
              {
                id: 'test',
                label: 'Parameter',
                minimum: 0,
                maximum: 1,
                step: 0.1,
                value: 0,
                suffix: '',
              },
            ]}
            onParameterChange={vi.fn()}
            title="Read only"
            category="Gain"
            icon="gain"
            inputCount={0}
            outputCount={0}
            channelCount={0}
            gainControl={{ label: 'Volume', valueDb: 0 }}
          />
        </MidiNodeContext.Provider>
      </MidiGraphContext.Provider>
    </MidiContext.Provider>,
  );
  expect(screen.queryByRole('button', { name: /Map .*Volume/ })).not.toBeInTheDocument();
  absent.unmount();
  const port = midiPortFixture();
  const command = vi.spyOn(port, 'command');
  access(port);
  await advance();
  const { fireEvent } = await import('@testing-library/react');
  fireEvent.keyDown(window, { key: 'm', ctrlKey: true, repeat: true });
  fireEvent.keyDown(window, { key: 'm', ctrlKey: true, isComposing: true });
  expect(command).not.toHaveBeenCalledWith('midi_map', expect.anything());
});

it('native MIDI menu events use the current mode and release the editor subscription', async () => {
  let nativeToggle = () => {};
  const stop = vi.fn();
  const port = {
    ...midiPortFixture(),
    subscribeMapToggle(listener: () => void) {
      nativeToggle = listener;
      return stop;
    },
  };
  const rendered = access(port);
  await advance();
  const { act } = await import('@testing-library/react');
  await act(async () => nativeToggle());
  expect(screen.getByRole('button', { name: 'MIDI map mode' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  await act(async () => nativeToggle());
  expect(screen.getByRole('button', { name: 'MIDI map mode' })).toHaveAttribute(
    'aria-pressed',
    'false',
  );
  rendered.unmount();
  expect(stop).toHaveBeenCalledOnce();
});

it('the panel follows capture and device reattachment observations without reopening or exposing IDs', async () => {
  const port = midiPortFixture({ ...midiFixture, devices: [] });
  access(port);
  await click('MIDI map mode');
  expect(await find('button', { name: 'Select Programme / Microphone · Volume' })).toBeVisible();
  expect(screen.getByText('Device disconnected')).toBeVisible();
  port.setStatus({
    ...midiFixture,
    mapMode: true,
    bindings: [
      midiBinding,
      {
        ...midiBinding,
        target: { graphId: 'main', nodeId: 'second' },
        nodeName: 'Programme output',
      },
    ],
  });
  await advance(250);
  expect(screen.queryByText('Device disconnected')).not.toBeInTheDocument();
  expect(
    screen.getByRole('button', { name: 'Select Programme / Programme output · Volume' }),
  ).toHaveTextContent('Programme / Programme output · Volume');
});
it('map mode disables non-mappable Options while preserving its panel relationship', async () => {
  const { MidiContext } = await import('../../core/midi/MidiContext');
  const { NodeOptionsSection } = await import('../../core/ui/NodeOptionsSection');
  view(
    <MidiContext.Provider
      value={{
        active: true,
        status: midiFixture,
        selected: null,
        toggle: vi.fn(),
        assign: vi.fn(),
        remove: vi.fn(),
      }}
    >
      <NodeOptionsSection label="Recorder options" opened onToggle={vi.fn()}>
        Non-mappable settings
      </NodeOptionsSection>
    </MidiContext.Provider>,
  );
  const options = screen.getByRole('button', { name: 'Recorder options' });
  expect(options).toBeDisabled();
  const panel = options.getAttribute('aria-controls');
  if (!panel) throw new Error('Options relationship required');
  expect(document.getElementById(panel)).toHaveAttribute('inert');
});

it('a saved recorder action keeps the selected mapping and keyboard focus across its identity change', async () => {
  const record = { ...midiBinding, action: 'record' as const };
  let status: MidiStatus = { ...midiFixture, mapMode: true, bindings: [record] };
  const port: MidiPort = {
    async command(name, args) {
      if (name === 'midi_save') {
        status = { ...status, bindings: [args?.binding as MidiBinding] };
      }
      return structuredClone(status);
    },
  };
  access(port);
  await click('MIDI map mode');
  await click('Select Programme / Microphone · Record / resume');
  screen.getByRole('button', { name: /Recorder action/ }).focus();
  await keyboard(' {End}{Enter}');
  const selected = await find('button', { name: 'Select Programme / Microphone · Stop' });
  expect(selected).toHaveAttribute('aria-pressed', 'true');
  expect(selected).toHaveFocus();
  expect(screen.getByRole('button', { name: /Recorder action/ })).toBeVisible();
});

it('a failed option save retains the selected mapping and reports the error', async () => {
  access({
    async command(name) {
      if (name === 'midi_save') throw new Error('disk unavailable');
      return structuredClone({ ...midiFixture, mapMode: true });
    },
  });
  await click('MIDI map mode');
  await click('Select Programme / Microphone · Volume');
  screen.getByRole('button', { name: /Volume mode/ }).focus();
  await keyboard(' {End}{Enter}');
  expect(await find('alert')).toHaveTextContent('Could not update MIDI controls');
  expect(
    screen.getByRole('button', { name: 'Select Programme / Microphone · Volume' }),
  ).toHaveAttribute('aria-pressed', 'true');
});

it('groups a shared hardware control once and uses each mapping title as its only selection focus stop', async () => {
  const second = {
    ...midiBinding,
    target: { graphId: 'main', nodeId: 'other' },
    nodeName: 'Programme output',
  };
  view(
    <MidiBindingsList
      status={{ ...midiFixture, bindings: [midiBinding, second] }}
      busy={false}
      selected={null}
      onSelect={vi.fn()}
      onChange={vi.fn()}
      onRemove={vi.fn()}
    />,
  );
  const group = screen.getByRole('region', { name: 'Studio Controller · Channel 1 · CC 7' });
  expect(
    within(group)
      .getAllByRole('button')
      .map((button) => button.textContent),
  ).toEqual([
    'Programme / Microphone · Volume',
    'Remove',
    'Programme / Programme output · Volume',
    'Remove',
  ]);
  expect(
    screen.queryByRole('heading', { name: 'Programme / Microphone · Volume' }),
  ).not.toBeInTheDocument();
  for (const name of [
    'Select Programme / Microphone · Volume',
    'Remove MIDI control for Programme / Microphone',
    'Select Programme / Programme output · Volume',
    'Remove MIDI control for Programme / Programme output',
  ]) {
    await user.tab();
    expect(screen.getByRole('button', { name })).toHaveFocus();
  }
});
