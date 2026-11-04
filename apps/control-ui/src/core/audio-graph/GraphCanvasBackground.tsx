import { Background, BackgroundVariant, Panel as ReactFlowPanel } from '@xyflow/react';
import { useTranslation } from 'react-i18next';
import { useMidiMappingActive } from '../midi/MidiContext';
import { HierarchicalMenuButton } from '../ui/HierarchicalMenu';
import styles from './AudioGraphCanvas.module.css';
import type { AudioGraphNodeMenuItem } from './audioGraphCanvasTypes';
import type { NodeMenu } from './useNodeMenu';

interface AddNodePanelProps {
  addNode: (itemId: string) => void;
  label?: string;
  nodeMenu: NodeMenu;
  nodeMenuItems: readonly AudioGraphNodeMenuItem[];
}

function AddNodePanel({ addNode, label, nodeMenu, nodeMenuItems }: AddNodePanelProps) {
  const { t } = useTranslation();
  const actionLabel = label?.trim() || t('routing.addNode');
  const mapping = useMidiMappingActive();
  if (nodeMenuItems.length === 0 || mapping) return null;
  return (
    <ReactFlowPanel className={`${styles.nodeToolbar} nodrag nopan nowheel`} position="top-left">
      <HierarchicalMenuButton
        buttonClassName={`${styles.addNode} nodrag nopan nowheel`}
        buttonContent={<span aria-hidden="true">+</span>}
        items={nodeMenuItems}
        label={actionLabel}
        tooltip={actionLabel}
        onOpenedChange={nodeMenu.openButtonMenu}
        onSelect={addNode}
        opened={nodeMenu.buttonOpened}
      />
    </ReactFlowPanel>
  );
}

export function GraphCanvasBackground({ empty, ...panel }: AddNodePanelProps & { empty: boolean }) {
  const { t } = useTranslation();
  return (
    <>
      <Background color="var(--canvas-grid)" gap={24} size={1} variant={BackgroundVariant.Lines} />
      {/* Guidance only: the + panel stays the one owner of the add-node action. */}
      {empty && panel.nodeMenuItems.length > 0 ? (
        <p className={styles.emptyHint}>{t('routing.emptyCanvas')}</p>
      ) : null}
      <AddNodePanel {...panel} />
    </>
  );
}
