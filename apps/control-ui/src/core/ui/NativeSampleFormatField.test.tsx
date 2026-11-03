import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { NativeDeviceFormats, NativeStreamFormat } from '../domain/control/nativeFormat';

import { MixRelayProvider } from '../theme/MixRelayProvider';
import { NativeSampleFormatField } from './NativeSampleFormatField';

const float: NativeStreamFormat = { sampleRate: 48000, channels: 2, sampleFormat: 'float32' };
const pcm: NativeStreamFormat = { ...float, sampleFormat: 'signed24' };
const formats: NativeDeviceFormats = {
  render: { current: float, supported: [float, pcm] },
  capture: { current: float, supported: [float, pcm] },
};

function fixture(
  options: { formats?: NativeDeviceFormats; mixedLabel?: string; disabled?: boolean } = {},
) {
  const onChange = vi.fn();
  render(
    <MixRelayProvider>
      <NativeSampleFormatField
        formats={formats}
        render
        capture
        label="PCM encoding"
        unknownLabel="Unknown format"
        onChange={onChange}
        {...options}
      />
    </MixRelayProvider>,
  );
  return { onChange, button: screen.getByRole('button', { name: /PCM encoding/ }) };
}

describe('device PCM field', () => {
  it('offers all supported encodings and keeps the observed native value until the owner confirms a change', async () => {
    const { button, onChange } = fixture();
    expect(button).toHaveTextContent('32-bit Float');
    fireEvent.click(button);
    fireEvent.click(await screen.findByRole('option', { name: '24-bit PCM' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith('signed24');
    expect(button).toHaveTextContent('32-bit Float');
  });

  it('does not send a second native mutation when the current format is reselected', async () => {
    const { button, onChange } = fixture();
    fireEvent.click(button);
    fireEvent.click(await screen.findByRole('option', { name: '32-bit Float' }));
    expect(onChange).not.toHaveBeenCalled();
  });

  it.each([undefined, 'Different direction formats'])(
    'shows mixed direction readback with placeholder %s',
    (mixedLabel) => {
      const { button } = fixture({
        mixedLabel,
        formats: { ...formats, capture: { current: pcm, supported: [float, pcm] } },
      });
      expect(button).toHaveTextContent(mixedLabel ?? 'Unknown format');
      expect(button).toBeEnabled();
    },
  );

  it.each([
    { disabled: true },
    {
      formats: {
        render: { current: null, supported: [] },
        capture: { current: null, supported: [] },
      },
    },
    {
      formats: {
        render: { current: float, supported: [float] },
        capture: { current: float, supported: [float] },
      },
    },
  ])('disables a field without actionable choices %j', (options) => {
    const { button } = fixture(options);
    expect(button).toBeDisabled();
  });

  it('retains an unsupported observed format as a disabled choice while allowing a supported replacement', async () => {
    const { button, onChange } = fixture({
      formats: {
        render: { current: float, supported: [pcm] },
        capture: { current: float, supported: [pcm] },
      },
    });
    fireEvent.click(button);
    const observed = await screen.findByRole('option', { name: '32-bit Float' });
    expect(observed).toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(observed);
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('option', { name: '24-bit PCM' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith('signed24');
  });
});
