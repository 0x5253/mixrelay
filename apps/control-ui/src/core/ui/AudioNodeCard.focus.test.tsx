import { fireEvent, render, screen } from '@testing-library/react';
import { Fragment, useLayoutEffect, useState } from 'react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard } from './AudioNodeCard';
import { NodePowerButton } from './NodeSignalControls';

const base = {
  title: 'Effect',
  icon: 'FX',
  category: 'Processor',
  inputCount: 2,
  outputCount: 2,
  faceMode: 'compact' as const,
};
const bypass = {
  id: 'bypass',
  label: 'Bypass',
  value: 0,
  minimum: 0,
  maximum: 1,
  step: 1,
  suffix: '',
  choices: [
    { value: 0, label: 'Processing' },
    { value: 1, label: 'Bypassed' },
  ],
};
function Recovery({
  unmute = false,
  disabledBypass = false,
}: {
  unmute?: boolean;
  disabledBypass?: boolean;
}) {
  const [off, setOff] = useState(true);
  const [muted, setMuted] = useState(unmute);
  return (
    <AudioNodeCard
      {...base}
      poweredOff={off && disabledBypass}
      headerActions={
        off ? (
          <button type="button" onClick={() => setOff(false)}>
            Enable
          </button>
        ) : undefined
      }
      muteControl={
        muted ? (
          <button type="button" onClick={() => setMuted(false)}>
            Unmute
          </button>
        ) : undefined
      }
      parameterControls={[bypass]}
      onParameterChange={vi.fn()}
    />
  );
}
it('keeps bypass named and executable when its numeric contract has no choice labels', () => {
  const change = vi.fn();
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        {...base}
        parameterControls={[{ ...bypass, choices: undefined }]}
        onParameterChange={change}
      />
    </MixRelayProvider>,
  );
  const processing = screen.getByRole('button', { name: (name) => name.endsWith('Bypass: 0') });
  expect(processing).toHaveAttribute('aria-pressed', 'false');
  processing.focus();
  fireEvent.click(processing);
  expect(change).toHaveBeenCalledExactlyOnceWith('bypass', 1);
  view.rerender(
    <MixRelayProvider>
      <AudioNodeCard
        {...base}
        parameterControls={[{ ...bypass, choices: undefined, value: 1 }]}
        onParameterChange={change}
      />
    </MixRelayProvider>,
  );
  const bypassed = screen.getByRole('button', { name: (name) => name.endsWith('Bypass: 1') });
  expect(bypassed).toHaveFocus();
  expect(bypassed).toHaveAttribute('aria-pressed', 'true');
  fireEvent.click(bypassed);
  expect(change).toHaveBeenLastCalledWith('bypass', 0);
});
it('returns the removed Enable and Unmute focus to the next remaining enabled header command', () => {
  render(
    <MixRelayProvider>
      <Recovery unmute />
    </MixRelayProvider>,
  );
  const enable = screen.getByRole('button', { name: 'Enable' });
  enable.focus();
  fireEvent.click(enable);
  const unmute = screen.getByRole('button', { name: 'Unmute' });
  expect(unmute).toHaveFocus();
  fireEvent.click(unmute);
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Processing') }),
  ).toHaveFocus();
});
it('focuses bypass after a successful Off recovery instead of losing focus to body', () => {
  render(
    <MixRelayProvider>
      <Recovery disabledBypass />
    </MixRelayProvider>,
  );
  const enable = screen.getByRole('button', { name: 'Enable' });
  enable.focus();
  fireEvent.click(enable);
  expect(
    screen.getByRole('button', { name: (name) => name.endsWith('Bypass: Processing') }),
  ).toHaveFocus();
});
it('retains the recovery owner through native disabled blur until the acknowledgement removes it', () => {
  const props = { ...base, parameterControls: [bypass], onParameterChange: vi.fn() };
  const action = (disabled: boolean) => (
    <NodePowerButton enabled={false} disabled={disabled} label="Enable Effect" onChange={vi.fn()} />
  );
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard {...props} poweredOff headerActions={action(false)} />
    </MixRelayProvider>,
  );
  const enable = screen.getByRole('button', { name: 'Enable Effect' });
  enable.focus();
  view.rerender(
    <MixRelayProvider>
      <AudioNodeCard {...props} poweredOff headerActions={action(true)} />
    </MixRelayProvider>,
  );
  // jsdom retains disabled focus. Model the browser's body focus during this
  // event only; acknowledgement and restoration below use the real DOM.
  const active = vi.spyOn(document, 'activeElement', 'get').mockReturnValue(document.body);
  fireEvent.blur(enable, { relatedTarget: document.body });
  active.mockRestore();
  view.rerender(
    <MixRelayProvider>
      <AudioNodeCard {...props} />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('button', { name: 'Effect: Bypass: Processing' })).toHaveFocus();
});
it('does not take focus from an outside action or after the user voluntarily blurs', () => {
  const props = {
    ...base,
    headerActions: <button type="button">Enable</button>,
    parameterControls: [bypass],
    onParameterChange: vi.fn(),
  };
  const view = render(
    <>
      <button type="button">Outside</button>
      <AudioNodeCard {...props} />
    </>,
  );
  screen.getByRole('button', { name: 'Enable' }).focus();
  screen.getByRole('button', { name: 'Outside' }).focus();
  view.rerender(
    <>
      <button type="button">Outside</button>
      <AudioNodeCard {...props} headerActions={undefined} />
    </>,
  );
  expect(screen.getByRole('button', { name: 'Outside' })).toHaveFocus();
  view.rerender(
    <>
      <button type="button">Outside</button>
      <AudioNodeCard {...props} />
    </>,
  );
  screen.getByRole('button', { name: 'Enable' }).focus();
  screen.getByRole('button', { name: 'Enable' }).blur();
  view.rerender(
    <>
      <button type="button">Outside</button>
      <AudioNodeCard {...props} headerActions={undefined} />
    </>,
  );
  expect(document.body).toHaveFocus();
});
it('skips disabled remaining commands and uses the previous neighbor if the last focused command disappears', () => {
  const props = {
    ...base,
    headerActions: (
      <>
        <button type="button" disabled>
          Unavailable
        </button>
        <button type="button" aria-disabled="true">
          Busy
        </button>
        <button type="button">Remaining</button>
        <button type="button">Remove</button>
      </>
    ),
  };
  const view = render(<AudioNodeCard {...props} />);
  screen.getByRole('button', { name: 'Remove' }).focus();
  view.rerender(
    <AudioNodeCard
      {...props}
      headerActions={
        <>
          <button type="button" disabled>
            Unavailable
          </button>
          <button type="button">Remaining</button>
        </>
      }
    />,
  );
  expect(screen.getByRole('button', { name: 'Remaining' })).toHaveFocus();
});
it('keeps focus where the user moved within the header and ignores a nonbutton header input', () => {
  const props = {
    ...base,
    headerActions: (
      <Fragment key="header">
        <button type="button">Enable</button>
        <input key="field" aria-label="Header input" />
      </Fragment>
    ),
    parameterControls: [bypass],
    onParameterChange: vi.fn(),
  };
  const view = render(<AudioNodeCard {...props} />);
  screen.getByRole('button', { name: 'Enable' }).focus();
  screen.getByRole('textbox', { name: 'Header input' }).focus();
  view.rerender(
    <AudioNodeCard
      {...props}
      headerActions={
        <Fragment key="header">
          <input key="field" aria-label="Header input" />
        </Fragment>
      }
    />,
  );
  expect(screen.getByRole('textbox', { name: 'Header input' })).toHaveFocus();
});
it('does not invent a focus target after the final available header action disappears', () => {
  const view = render(
    <AudioNodeCard {...base} headerActions={<button type="button">Enable</button>} />,
  );
  screen.getByRole('button', { name: 'Enable' }).focus();
  view.rerender(<AudioNodeCard {...base} />);
  expect(document.body).toHaveFocus();
});

it('respects another focus owner that acts after the old recovery button is removed', () => {
  function OtherFocusOwner({ restore }: { restore: boolean }) {
    useLayoutEffect(() => {
      if (restore) document.getElementById('outside-owner')?.focus();
    }, [restore]);
    return (
      <button type="button" id="outside-owner">
        Other focus owner
      </button>
    );
  }
  function Example() {
    const [off, setOff] = useState(true);
    return (
      <>
        <OtherFocusOwner restore={!off} />
        <AudioNodeCard
          {...base}
          parameterControls={[bypass]}
          onParameterChange={vi.fn()}
          headerActions={
            off ? (
              <button type="button" onClick={() => setOff(false)}>
                Enable
              </button>
            ) : undefined
          }
        />
      </>
    );
  }
  render(<Example />);
  const enable = screen.getByRole('button', { name: 'Enable' });
  enable.focus();
  fireEvent.click(enable);
  expect(screen.getByRole('button', { name: 'Other focus owner' })).toHaveFocus();
});
