import { border, sizes, spacing } from '../theme/tokens';

/** Core node frame shared by cards, utility rows and cable anchors. Rows use the shared
 * size steps; the collapsed default face stays 120px including borders. */
export const audioNodeFrame = {
  headerHeight: sizes.sm,
  utilityHeight: sizes.xs,
  minimumBodyHeight: 66,
  borderWidth: border.width,
};

/** Face geometry shared by the card and cable anchors. Options never take part
 * in this calculation. Keep these metrics in sync with AudioNodeCard.module.css. */
export function minimumAudioNodeFaceHeight(
  meterBanks: readonly { channels: number; label?: string }[] = [],
) {
  const { bodyPadding, meterMargin, rowHeight, rowGap } = channelRowMetrics;
  // A grid cell is the 24px bar and 9px index separated by the half step.
  const cell = 24 + 9 + spacing.half;
  // The 11px caption line and the bank's top margin.
  const bankLabelHeight = 11 + meterMargin;
  const banks = meterBanks.filter((bank) => bank.channels > 0);
  const meters = banks.reduce((total, bank) => {
    const count = Math.min(64, Math.max(1, Math.trunc(bank.channels)));
    const rows = Math.ceil(count / (count > 32 ? 16 : 8));
    const content =
      (count <= 8
        ? count * rowHeight + (count - 1) * rowGap
        : rows * cell + (rows - 1) * spacing[1]) + meterMargin;
    return total + content + (banks.length > 1 && bank.label ? bankLabelHeight : 0);
  }, 0);
  // Metadata shares the utility row; only meters expand the body.
  const body = Math.max(
    audioNodeFrame.minimumBodyHeight,
    bodyPadding * 2 + meters + Math.max(0, banks.length - 1) * rowGap,
  );
  return audioNodeFrame.headerHeight + body + audioNodeFrame.utilityHeight;
}

export const channelRowMetrics = {
  headerHeight: audioNodeFrame.headerHeight,
  bodyPadding: 7,
  meterMargin: spacing.half,
  rowHeight: 10,
  rowGap: 3,
  borderWidth: audioNodeFrame.borderWidth,
};

export type AudioNodeFaceMode = 'standard' | 'channel-rows' | 'compact';

export interface AudioNodeFaceGeometry {
  height: number;
  firstChannelCenter: number | null;
  channelPitch: number;
  portInsetTop?: number;
  portInsetBottom?: number;
}

/** Channel-row faces contain only their header and meter rows. The same row
 * metrics drive CSS and cable centers; disclosure content is outside this face. */
export function audioNodeFaceGeometry({
  meterBanks = [],
  portCount = 0,
  mode = 'standard',
}: {
  meterBanks?: readonly { channels: number; label?: string }[];
  portCount?: number;
  mode?: AudioNodeFaceMode;
}): AudioNodeFaceGeometry {
  if (mode === 'compact')
    return {
      height: channelRowMetrics.headerHeight,
      firstChannelCenter: null,
      channelPitch: 0,
    };
  if (mode === 'standard')
    return {
      height: Math.max(
        minimumAudioNodeFaceHeight(meterBanks),
        audioNodeFrame.headerHeight + audioNodeFrame.utilityHeight + (portCount + 1) * 12,
      ),
      firstChannelCenter: null,
      channelPitch: 0,
      portInsetTop: audioNodeFrame.headerHeight,
      portInsetBottom: audioNodeFrame.utilityHeight,
    };
  const { headerHeight, bodyPadding, meterMargin, rowHeight, rowGap, borderWidth } =
    channelRowMetrics;
  const count = Math.max(1, Math.trunc(meterBanks[0]?.channels ?? 1));
  const rowsHeight = count * rowHeight + (count - 1) * rowGap;
  const bodyHeight = Math.max(
    audioNodeFrame.minimumBodyHeight,
    bodyPadding * 2 + meterMargin + rowsHeight,
  );
  const meterStart =
    headerHeight +
    bodyPadding +
    meterMargin +
    (bodyHeight - bodyPadding * 2 - meterMargin - rowsHeight) / 2;
  return {
    height: headerHeight + bodyHeight,
    firstChannelCenter: borderWidth + meterStart + rowHeight / 2,
    channelPitch: rowHeight + rowGap,
  };
}

export function audioNodePortTop(geometry: AudioNodeFaceGeometry, index: number, count: number) {
  const top = geometry.portInsetTop ?? 0;
  const available = geometry.height - top - (geometry.portInsetBottom ?? 0);
  return geometry.firstChannelCenter === null
    ? top + ((index + 1) / (count + 1)) * available
    : geometry.firstChannelCenter + index * geometry.channelPitch;
}
