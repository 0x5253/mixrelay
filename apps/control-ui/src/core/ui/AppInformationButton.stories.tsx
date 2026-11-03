import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';
import { AppInformationButton } from './AppInformationButton';
import { ApplicationStatusBar } from './ApplicationStatusBar';
import { TransportButton } from './TransportButton';

const meta = {
  title: 'Core/Application/App Information',
  component: AppInformationButton,
  args: {
    label: 'About MixRelay',
    version: '0.1.3',
    opened: false,
    onOpen: fn(),
  },
  render: (args) => (
    <ApplicationStatusBar
      information={<AppInformationButton {...args} />}
      controls={
        <TransportButton
          icon="pause"
          label="Pause"
          description="Audio session running"
          compact
          disabled={false}
          onClick={fn()}
        />
      }
    />
  ),
} satisfies Meta<typeof AppInformationButton>;

export default meta;
type Story = StoryObj<typeof meta>;
export const Default: Story = {};
export const Opened: Story = { args: { opened: true } };
export const Keyboard: Story = {
  play: async ({ canvasElement, args }) => {
    const button = within(canvasElement).getByRole('button', { name: args.label });
    button.focus();
    await userEvent.keyboard('{Enter}');
    await userEvent.keyboard(' ');
    await expect(args.onOpen).toHaveBeenCalledTimes(2);
    await expect(button).toHaveAttribute('aria-haspopup', 'dialog');
    await expect(button).toHaveAttribute('aria-description', args.version);
    await userEvent.tab();
    await expect(within(canvasElement).getByRole('button', { name: 'Pause' })).toHaveFocus();
  },
};
