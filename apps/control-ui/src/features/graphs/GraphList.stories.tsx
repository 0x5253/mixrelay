import type { Meta, StoryObj } from '@storybook/react-vite';
import { useEffect, useState } from 'react';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';
import { i18n } from '../../core/i18n';
import { GraphList } from './GraphList';

const meta = {
  title: 'Features/Graphs/List',
  component: GraphList,
  args: {
    catalog: {
      protocolVersion: 1,
      catalogRevision: 1,
      activeId: 'studio',
      document: { schemaVersion: 2, revision: 1, nodes: [], edges: [] },
      graphs: [
        { id: 'studio', name: 'Studio' },
        { id: 'live', name: 'Live' },
      ],
    },
    disabled: false,
    busy: false,
    error: null,
    onSelect: fn(),
    onDelete: fn(),
  },
  argTypes: { onSelect: { control: false }, onDelete: { control: false } },
  render: function List(args, context) {
    const [catalog, setCatalog] = useState(args.catalog);
    const [working, setWorking] = useState(false);
    const [error, setError] = useState(args.error);
    useEffect(() => setCatalog(args.catalog), [args.catalog]);
    useEffect(() => setError(args.error), [args.error]);
    return (
      <GraphList
        {...args}
        catalog={catalog}
        busy={args.busy || working}
        disabled={args.disabled || working}
        error={error}
        onSelect={(id) => {
          args.onSelect(id);
          setCatalog({ ...catalog, activeId: id });
        }}
        onDelete={(id) => {
          args.onDelete(id);
          if (context.parameters.deletePending) {
            setWorking(true);
            return;
          }
          if (context.parameters.deleteFailed) {
            setError('삭제하지 못했습니다. 목록과 선택은 유지합니다.');
            return;
          }
          const graphs = catalog.graphs.filter((graph) => graph.id !== id);
          setCatalog({
            ...catalog,
            graphs,
            activeId: catalog.activeId === id ? (graphs[0]?.id ?? '') : catalog.activeId,
          });
        }}
      />
    );
  },
} satisfies Meta<typeof GraphList>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Selected: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const DeleteConfirmation: Story = {
  play: async ({ canvasElement }) => {
    const buttons = within(canvasElement).getAllByRole('button', {
      name: (name) => name.endsWith(i18n.t('studio.deleteGraph')),
    });
    const button = buttons[0];
    if (!button) throw new Error('Delete fixture requires a graph');
    await userEvent.click(button);
    const dialog = await within(canvasElement.ownerDocument.body).findByRole('dialog');
    await waitFor(() => expect(dialog).toBeVisible());
  },
};
export const DeleteFailed: Story = {
  parameters: { deleteFailed: true },
  play: async (context) => {
    await DeleteConfirmation.play?.(context);
    const dialog = await within(context.canvasElement.ownerDocument.body).findByRole('dialog');
    await userEvent.click(
      within(dialog).getByRole('button', {
        name: (name) => name.endsWith(i18n.t('studio.deleteGraph')),
      }),
    );
    const alert = await within(dialog).findByRole('alert');
    await waitFor(() => expect(alert).toBeVisible());
  },
};
export const DeletePending: Story = {
  parameters: { deletePending: true },
  play: async (context) => {
    await DeleteConfirmation.play?.(context);
    const dialog = await within(context.canvasElement.ownerDocument.body).findByRole('dialog');
    const confirm = within(dialog).getByRole('button', {
      name: (name) => name.endsWith(i18n.t('studio.deleteGraph')),
    });
    await userEvent.click(confirm);
    await expect(confirm).toBeDisabled();
    await userEvent.keyboard('{Escape}');
    await expect(dialog).toBeVisible();
  },
};
export const LongNames: Story = {
  args: {
    catalog: {
      ...meta.args.catalog,
      graphs: [
        { id: 'studio', name: 'Studio Monitor Mix · 매우 긴 그래프 이름' },
        { id: 'live', name: 'Live Stream · 녹음과 출력 장치의 작업 목록' },
      ],
    },
  },
};
