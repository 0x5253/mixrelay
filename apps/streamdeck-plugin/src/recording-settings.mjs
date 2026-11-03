import { translate } from './localization.mjs';

/** Display the desktop-owned recorder preferences without inventing or editing them. */
export function recordingSettingsSummary(node, language) {
  const settings = node?.recordingSettings;
  const t = (key) => translate(language, key);
  if (!settings) return { folder: t('Unavailable'), format: t('Unavailable') };
  const encoding = { pcm16: '16-bit PCM', pcm24: '24-bit PCM', float32: '32-bit float' }[
    settings.encoding
  ];
  const rate =
    settings.sampleRate === null
      ? t('Follow input')
      : Number.isFinite(settings.sampleRate) && settings.sampleRate > 0
        ? `${settings.sampleRate / 1000} kHz`
        : t('Unavailable');
  return {
    folder: settings.folder || t('Choose a recording folder in MixRelay.'),
    format: `${rate} · ${encoding ? t(encoding) : t('Unavailable')}`,
  };
}
