import { expect, it } from 'vitest';
import {
  audioNodeFaceGeometry,
  audioNodePortTop,
  channelRowMetrics,
  minimumAudioNodeFaceHeight,
} from './audioNodeLayout';

it.each(Array.from({ length: 64 }, (_, index) => index + 1))(
  'reserves the complete meter labels and bottom padding for %i channels',
  (channels) => {
    const rows = Math.ceil(channels / (channels > 32 ? 16 : 8));
    const content = channels <= 8 ? channels * 10 + (channels - 1) * 3 : rows * 35 + (rows - 1) * 4;
    expect(minimumAudioNodeFaceHeight([{ channels }]) - 28 - 24 - 14 - 2).toBeGreaterThanOrEqual(
      content,
    );
  },
);

it('keeps the normal stereo face compact and reserves space for the eighth label', () => {
  expect(minimumAudioNodeFaceHeight([])).toBe(118);
  expect(minimumAudioNodeFaceHeight([{ channels: 2 }])).toBe(118);
  expect(minimumAudioNodeFaceHeight([{ channels: 8 }])).toBe(169);
});

it.each([1, 2, 4])(
  'shares the 120px collapsed frame across normal and recorder nodes with %i channels',
  (channels) => {
    const normal = audioNodeFaceGeometry({ meterBanks: [{ channels }], portCount: channels });
    const recorder = audioNodeFaceGeometry({
      meterBanks: [{ channels }],
      portCount: channels,
      mode: 'channel-rows',
    });
    expect(normal.height + 2).toBe(120);
    expect(recorder.height + 24 + 2).toBe(120);
  },
);

it.each(Array.from({ length: 8 }, (_, index) => index + 1))(
  'fits %i recorder rows and anchors every cable at its meter center',
  (channels) => {
    const geometry = audioNodeFaceGeometry({
      meterBanks: [{ channels }],
      portCount: channels,
      mode: 'channel-rows',
    });
    const { headerHeight, bodyPadding, meterMargin, rowHeight, rowGap, borderWidth } =
      channelRowMetrics;
    expect(geometry.height).toBe(Math.max(94, 41 + channels * 13));
    for (let index = 0; index < channels; index += 1) {
      const center =
        borderWidth +
        headerHeight +
        bodyPadding +
        meterMargin +
        Math.max(0, 94 - (41 + channels * 13)) / 2 +
        rowHeight / 2 +
        index * (rowHeight + rowGap);
      expect(audioNodePortTop(geometry, index, channels)).toBe(center);
    }
    // A multi-cable preview must use the same row pitch as the actual handles.
    expect(
      audioNodePortTop(geometry, channels - 1, channels) - audioNodePortTop(geometry, 0, channels),
    ).toBe((channels - 1) * 13);
  },
);

it('preserves standard faces and their distributed port positions', () => {
  const stereo = audioNodeFaceGeometry({ meterBanks: [{ channels: 2 }], portCount: 2 });
  expect(stereo.height).toBe(118);
  expect(audioNodePortTop(stereo, 0, 2)).toBeCloseTo(28 + 66 / 3);
  expect(audioNodePortTop(stereo, 1, 2)).toBeCloseTo(28 + 132 / 3);
  expect(audioNodeFaceGeometry({ meterBanks: [{ channels: 64 }], portCount: 64 }).height).toBe(832);
});

it.each([
  { channels: 8, expectedHeight: 301 },
  { channels: 16, expectedHeight: 247 },
  { channels: 64, expectedHeight: 403 },
])(
  'reserves the full face for two labelled $channels-channel meter banks',
  ({ channels, expectedHeight }) => {
    const twoBanks = audioNodeFaceGeometry({
      meterBanks: [
        { channels, label: 'Main' },
        { channels, label: 'Sidechain' },
      ],
    });
    // Both labels, both meter margins, and the 3px CSS gap between bank groups.
    expect(twoBanks.height).toBe(expectedHeight);
  },
);

it.each([1, 2, 4, 8])('keeps an unmetered plug-in header compact with %i ports', (ports) => {
  const geometry = audioNodeFaceGeometry({ mode: 'compact', portCount: ports });
  expect(geometry.height).toBe(28);
  expect(geometry.height).toBeLessThan(minimumAudioNodeFaceHeight([{ channels: ports }]));
});

it.each([1, 2, 8])(
  'places %i plug-in ports below the header and above the resize grip',
  (count) => {
    const geometry = {
      ...audioNodeFaceGeometry({ mode: 'compact' }),
      height: 300,
      portInsetTop: 26,
      portInsetBottom: 12,
    };
    for (let index = 0; index < count; index++) {
      const top = audioNodePortTop(geometry, index, count);
      expect(top).toBeGreaterThan(26);
      expect(top).toBeLessThan(288);
    }
  },
);

it('uses one placeholder recorder row while channel metadata is unavailable', () => {
  const unresolved = audioNodeFaceGeometry({ mode: 'channel-rows', meterBanks: [] });
  const mono = audioNodeFaceGeometry({ mode: 'channel-rows', meterBanks: [{ channels: 1 }] });
  expect(unresolved).toEqual(mono);
  expect(audioNodePortTop(unresolved, 0, 1)).toBeGreaterThan(channelRowMetrics.headerHeight);
});
