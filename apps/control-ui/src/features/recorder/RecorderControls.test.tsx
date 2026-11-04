import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import type {
  RecorderRequest,
  RecorderSettings,
  RecorderStatus,
} from '../../core/domain/control/recorder';
import { I18nProvider, i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import type { SelectFieldProps } from '../../core/ui/SelectField';
import { RecorderControls } from './RecorderControls';

const selections = vi.hoisted(() => new Map<string, SelectFieldProps>());
vi.mock('../../core/ui/SelectField', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../core/ui/SelectField')>();
  return {
    ...actual,
    SelectField: (props: SelectFieldProps) => {
      selections.set(props.label, props);
      return <actual.SelectField {...props} />;
    },
  };
});

const stopped: RecorderStatus = {
  state: 'stopped',
  path: null,
  sampleRate: null,
  encoding: null,
  frames: 0,
  error: null,
};
function fixture() {
  const port = new StorybookMockControlPort({
    graph: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 256,
      underruns: 0,
      overruns: 0,
    },
  });
  let status = stopped;
  vi.spyOn(port, 'recorderStatus').mockImplementation(async () => status);
  const write = vi
    .spyOn(port, 'controlRecorder')
    .mockImplementation(async (_graph, _node, request: RecorderRequest) => {
      status =
        request.action === 'record'
          ? {
              ...status,
              state: 'recording',
              path: '/Desktop/engine-owned.wav',
              sampleRate: 48000,
              encoding: 'pcm16',
              frames: 96000,
            }
          : { ...status, state: request.action === 'pause' ? 'paused' : 'stopped' };
      return status;
    });
  const choose = vi.fn().mockResolvedValue('/Chosen');
  const native = Object.assign(port, { selectRecordingFolder: choose });
  const save = vi.spyOn(port, 'setRecordingSettings');
  return { port: native, write, choose, save, apply: vi.spyOn(port, 'applyGraph') };
}
function recorderView(
  port: StorybookMockControlPort,
  options: {
    channels?: number;
    enabled?: boolean;
    poweredOn?: boolean;
    playbackPaused?: boolean;
    graphId?: string;
    nodeId?: string;
    name?: string;
    onChannelsChange?: (channels: number) => void;
  } = {},
) {
  return (
    <MixRelayProvider>
      <I18nProvider>
        <RecorderControls
          controlPort={port}
          graphId={options.graphId ?? 'graph-a'}
          nodeId={options.nodeId ?? 'recorder-1'}
          name={options.name}
          expectedRevision={1}
          poweredOn={options.poweredOn ?? true}
          playbackPaused={options.playbackPaused}
          enabled={options.enabled ?? true}
          channels={options.channels ?? 2}
          onChannelsChange={options.onChannelsChange ?? vi.fn()}
        />
      </I18nProvider>
    </MixRelayProvider>
  );
}
it('names recording and folder actions by graph instance and exposes folder settings', async () => {
  const { port, write } = fixture();
  render(recorderView(port, { name: 'Music' }));
  const record = await screen.findByRole('button', { name: 'Music: Record' });
  expect(screen.getByLabelText('Recorded time')).toHaveAttribute('aria-live', 'off');
  await waitFor(() => expect(record).toBeEnabled());
  fireEvent.click(record);
  await waitFor(() =>
    expect(write).toHaveBeenLastCalledWith('graph-a', 'recorder-1', { action: 'record' }, 1),
  );
  const pause = screen.getByRole('button', { name: 'Music: Pause' });
  await waitFor(() => expect(pause).toBeEnabled());
  fireEvent.click(pause);
  const resume = await screen.findByRole('button', { name: 'Music: Resume' });
  await waitFor(() => expect(resume).toBeEnabled());
  fireEvent.click(resume);
  await waitFor(() =>
    expect(write).toHaveBeenLastCalledWith('graph-a', 'recorder-1', { action: 'record' }, 1),
  );
  const stop = screen.getByRole('button', { name: 'Music: Stop' });
  await waitFor(() => expect(stop).toBeEnabled());
  fireEvent.click(stop);
  await waitFor(() =>
    expect(write).toHaveBeenLastCalledWith('graph-a', 'recorder-1', { action: 'stop' }, 1),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Music: Options' }));
  expect(screen.getByRole('button', { name: 'Music: Choose folder…' })).toBeEnabled();
});

async function openOptions() {
  fireEvent.click(
    screen.getByRole('button', { name: (name) => name.endsWith(i18n.t('routing.nodes.options')) }),
  );
  await waitFor(() =>
    expect(screen.getByLabelText(i18n.t('recorder.folder'))).toHaveValue('/Desktop'),
  );
}
beforeEach(async () => {
  selections.clear();
  await i18n.changeLanguage('en');
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockReturnValue(
    new DOMRect(20, 20, 240, 32),
  );
  vi.spyOn(document.documentElement, 'clientWidth', 'get').mockReturnValue(1024);
  vi.spyOn(document.documentElement, 'clientHeight', 'get').mockReturnValue(768);
});

it('keeps nonstandard native sample-rate readback and can explicitly restore the source rate', async () => {
  const { port, save } = fixture();
  vi.spyOn(port, 'recordingSettings').mockResolvedValue({
    folder: '/Desktop',
    sampleRate: 32000,
    encoding: 'pcm16',
  });
  render(recorderView(port));
  await openOptions();
  expect(screen.getByRole('button', { name: /WAV sample rate$/ })).toHaveTextContent('32 kHz');
  fireEvent.click(screen.getByRole('button', { name: /WAV sample rate$/ }));
  fireEvent.click(await screen.findByRole('option', { name: 'Input rate' }));
  await waitFor(() =>
    expect(save).toHaveBeenCalledExactlyOnceWith(
      'graph-a',
      'recorder-1',
      { folder: '/Desktop', sampleRate: null, encoding: 'pcm16' },
      1,
    ),
  );
});

it('allows eight-channel and mono edits while rejecting an unsupported encoding', async () => {
  const { port, save } = fixture();
  const onChannelsChange = vi.fn();
  render(recorderView(port, { channels: 8, onChannelsChange }));
  await openOptions();
  fireEvent.click(screen.getByRole('button', { name: /Channels$/ }));
  const overLimit = await screen.findByRole('option', { name: '8' });
  expect(overLimit).not.toHaveAttribute('aria-disabled', 'true');
  fireEvent.click(overLimit);
  expect(onChannelsChange).toHaveBeenLastCalledWith(8);
  fireEvent.click(screen.getByRole('button', { name: /Channels$/ }));
  fireEvent.click(screen.getByRole('option', { name: '1' }));
  expect(onChannelsChange).toHaveBeenLastCalledWith(1);
  const channel = selections.get('Channels');
  const encoding = selections.get('Bit depth');
  if (!channel || !encoding) throw new Error('Recorder option fields must be mounted');
  // The Feature boundary also rejects unexpected values from its Core callback.
  act(() => {
    channel.onChange('8');
    encoding.onChange('unsupported');
  });
  expect(onChannelsChange).toHaveBeenCalledTimes(3);
  expect(onChannelsChange).toHaveBeenLastCalledWith(8);
  expect(save).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: /Bit depth$/ })).toHaveTextContent('16-bit PCM');
});

it('keeps canonical folder readback without a chooser when the native platform has no chooser capability', async () => {
  const { port } = fixture();
  render(recorderView(Object.assign(port, { selectRecordingFolder: undefined })));
  await openOptions();
  expect(screen.getByLabelText('Recording folder')).toHaveValue('/Desktop');
  expect(screen.getByLabelText('Recording folder')).toHaveAttribute('readonly');
  expect(screen.queryByRole('button', { name: 'Choose folder…' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Record' })).toBeEnabled();
});
afterEach(() => vi.restoreAllMocks());

it.each(['en', 'ko-KR', 'pseudo'])(
  'shows a readonly shared folder and native chooser in %s',
  async (locale) => {
    await i18n.changeLanguage(locale);
    const { port, choose, save, apply } = fixture();
    render(recorderView(port));
    await openOptions();
    const folder = screen.getByLabelText(i18n.t('recorder.folder'));
    expect(folder).toHaveAttribute('readonly');
    fireEvent.click(screen.getByRole('button', { name: i18n.t('recorder.chooseFolder') }));
    await waitFor(() => expect(folder).toHaveValue('/Chosen'));
    expect(choose).toHaveBeenCalledExactlyOnceWith({
      defaultPath: '/Desktop',
      title: i18n.t('recorder.chooseFolder'),
    });
    expect(save).toHaveBeenCalledExactlyOnceWith(
      'graph-a',
      'recorder-1',
      { folder: '/Chosen', sampleRate: null, encoding: 'pcm16' },
      1,
    );
    expect(apply).not.toHaveBeenCalled();
  },
);

it('keeps closed Options compact and sends action-only Record/Pause/Resume/Stop', async () => {
  const { port, write, apply } = fixture();
  render(recorderView(port));
  expect(screen.queryByLabelText('Recording folder')).not.toBeInTheDocument();
  const record = screen.getByRole('button', { name: 'Record' });
  await waitFor(() => expect(record).toBeEnabled());
  fireEvent.click(record);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Resume' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(4));
  expect(write.mock.calls.map((call) => call[2])).toEqual([
    { action: 'record' },
    { action: 'pause' },
    { action: 'record' },
    { action: 'stop' },
  ]);
  expect(apply).not.toHaveBeenCalled();
});

it('saves encoding and sample rate independently without graph mutation', async () => {
  const { port, save, apply } = fixture();
  render(recorderView(port));
  await openOptions();
  fireEvent.click(screen.getByRole('button', { name: /WAV sample rate$/ }));
  fireEvent.click(await screen.findByRole('option', { name: '96 kHz' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.getByRole('button', { name: /Bit depth$/ })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: /Bit depth$/ }));
  fireEvent.click(await screen.findByRole('option', { name: '24-bit PCM' }));
  await waitFor(() =>
    expect(save).toHaveBeenLastCalledWith(
      'graph-a',
      'recorder-1',
      { folder: '/Desktop', sampleRate: 96000, encoding: 'pcm24' },
      1,
    ),
  );
  expect(apply).not.toHaveBeenCalled();
});

it('blocks Record until the settings save completes and preserves folder after cancellation', async () => {
  const { port, choose, save, write } = fixture();
  let finish: (settings: RecorderSettings) => void = () => undefined;
  save.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = done;
      }),
  );
  render(recorderView(port));
  await openOptions();
  fireEvent.click(screen.getByRole('button', { name: 'Choose folder…' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(screen.getByRole('button', { name: 'Record' })).toBeDisabled();
  expect(write).not.toHaveBeenCalled();
  finish({ folder: '/Chosen', sampleRate: null, encoding: 'pcm16' });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Record' })).toBeEnabled());
  choose.mockResolvedValueOnce(null);
  fireEvent.click(screen.getByRole('button', { name: 'Choose folder…' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Record' })).toBeEnabled());
  expect(screen.getByLabelText('Recording folder')).toHaveValue('/Chosen');
  expect(save).toHaveBeenCalledTimes(1);
});

it('shows a save failure and permits an explicit chooser retry without recording an unsaved edit', async () => {
  const { port, save, write } = fixture();
  save.mockRejectedValueOnce(new Error('Disk full'));
  render(recorderView(port));
  await openOptions();
  fireEvent.click(screen.getByRole('button', { name: 'Choose folder…' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Disk full');
  expect(screen.getByRole('button', { name: 'Record' })).toBeDisabled();
  expect(write).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Choose folder…' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Record' })).toBeEnabled());
});

it('shows settings load failure without fabricating a folder', async () => {
  const { port } = fixture();
  vi.spyOn(port, 'recordingSettings').mockRejectedValue(new Error('Desktop unavailable'));
  render(recorderView(port));
  expect(await screen.findByRole('alert')).toHaveTextContent('Desktop unavailable');
  expect(screen.getByRole('button', { name: 'Record' })).toBeDisabled();
});

it('ignores a late folder selection when graph and node props are replaced', async () => {
  const { port, choose, save } = fixture();
  let finish: (folder: string) => void = () => undefined;
  choose.mockImplementationOnce(
    () =>
      new Promise((done) => {
        finish = done;
      }),
  );
  const result = render(recorderView(port));
  await openOptions();
  fireEvent.click(screen.getByRole('button', { name: 'Choose folder…' }));
  await waitFor(() => expect(choose).toHaveBeenCalledTimes(1));
  result.rerender(recorderView(port, { graphId: 'graph-b', nodeId: 'recorder-2' }));
  finish('/wrong');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Record' })).toBeEnabled());
  expect(save).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Recording folder')).toHaveValue('/Desktop');
});

it('preserves channels 1–8 and locks channel/settings edits for active takes', async () => {
  const { port } = fixture();
  const onChannelsChange = vi.fn();
  render(recorderView(port, { onChannelsChange }));
  await openOptions();
  fireEvent.click(screen.getByRole('button', { name: /Channels$/ }));
  const options = await screen.findAllByRole('option');
  expect(options.map((option) => option.textContent)).toEqual([
    '1',
    '2',
    '3',
    '4',
    '5',
    '6',
    '7',
    '8',
  ]);
  fireEvent.click(screen.getByRole('option', { name: '8' }));
  expect(onChannelsChange).toHaveBeenCalledWith(8);
  fireEvent.click(screen.getByRole('button', { name: 'Record' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled());
  for (const label of ['Channels', 'WAV sample rate', 'Bit depth'])
    expect(screen.getByRole('button', { name: (name) => name.endsWith(label) })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Choose folder…' })).toBeDisabled();
});

it('keeps Stop available and shows actual take format when canonical preferences cannot be read', async () => {
  const { port, write } = fixture();
  vi.spyOn(port, 'recorderStatus').mockResolvedValue({
    ...stopped,
    state: 'recording',
    sampleRate: 48000,
    encoding: 'pcm24',
  });
  vi.spyOn(port, 'recordingSettings').mockRejectedValue(new Error('Preferences unavailable'));
  render(recorderView(port));
  expect(await screen.findByRole('alert')).toHaveTextContent('Preferences unavailable');
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Options') }));
  expect(screen.getByRole('button', { name: /WAV sample rate$/ })).toHaveTextContent('48 kHz');
  expect(screen.getByRole('button', { name: /Bit depth$/ })).toHaveTextContent('24-bit PCM');
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
  await waitFor(() =>
    expect(write).toHaveBeenCalledExactlyOnceWith('graph-a', 'recorder-1', { action: 'stop' }, 1),
  );
});
