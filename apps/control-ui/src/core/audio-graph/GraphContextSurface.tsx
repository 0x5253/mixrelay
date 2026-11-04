import type { ReactNode } from 'react';
import { useMidiMappingActive } from '../midi/MidiContext';
import { HierarchicalContextMenu } from '../ui/HierarchicalMenu';
import styles from './AudioGraphCanvas.module.css';
import { graphContextMenuAnchor, handleGraphKeyDown } from './graphKeyboard';
import type { useAudioPortSelection } from './useAudioPortSelection';
import type { useGraphContextMenu } from './useGraphContextMenu';
import type { NodeMenu } from './useNodeMenu';

/** The focusable canvas region: it owns the context menu, the canvas keyboard commands
 * and the pointer capture that drives port selection. */
export function GraphContextSurface({
  children,
  contextMenu,
  nodeMenu,
  addNode,
  ariaLabel,
  captureProps,
  renameNode,
}: {
  children: ReactNode;
  contextMenu: ReturnType<typeof useGraphContextMenu>;
  nodeMenu: NodeMenu;
  addNode: (id: string) => void;
  ariaLabel: string;
  captureProps: ReturnType<typeof useAudioPortSelection>['captureProps'];
  renameNode?: (nodeId: string) => void;
}) {
  const mapping = useMidiMappingActive();
  return (
    <HierarchicalContextMenu
      getAnchor={graphContextMenuAnchor}
      items={mapping ? [] : contextMenu.items}
      onOpenedChange={nodeMenu.setContextOpened}
      onSelect={(id) => {
        nodeMenu.close();
        if (!contextMenu.execute(id)) addNode(id);
      }}
      opened={nodeMenu.contextOpened && !mapping}
    >
      <section
        {...captureProps}
        onKeyDownCapture={(event) =>
          !mapping &&
          handleGraphKeyDown(event, {
            capturePortKeys: captureProps.onKeyDownCapture,
            clearSelection: contextMenu.clearSelection,
            renameNode,
          })
        }
        onPointerDownCapture={(event) => {
          // The whole canvas is the context trigger, so the menu's outside-click
          // handler cannot dismiss clicks on other nodes or ports inside it.
          if (event.button !== 2) nodeMenu.setContextOpened(false);
          if (!mapping) captureProps.onPointerDownCapture(event);
        }}
        aria-label={ariaLabel}
        tabIndex={-1}
        className={styles.root}
        data-midi-mode={mapping || undefined}
        ref={nodeMenu.rootRef}
        onContextMenu={(event) => {
          contextMenu.prepare(event);
          const point = graphContextMenuAnchor(event);
          nodeMenu.prepareContextPosition({ clientX: point.x, clientY: point.y });
          // React normalizes mouse events on text nodes to their parent Element.
          const owner = (event.target as Element).closest<HTMLElement>(
            '.react-flow__node, .react-flow__edge',
          );
          owner?.focus();
        }}
        onTouchStart={(event) => {
          // the context trigger's long-press opens without dispatching a contextmenu event.
          // Capture the target and position for that path as well.
          const touch = event.touches[0];
          if (!touch) return;
          contextMenu.prepare(event);
          nodeMenu.prepareContextPosition(touch);
        }}
        onContextMenuCapture={(event) => {
          // Native text editing menus must not act on the enclosing node.
          if (
            event.target instanceof Element &&
            event.target.closest('input, textarea, [contenteditable="true"]')
          )
            event.stopPropagation();
        }}
      >
        {children}
      </section>
    </HierarchicalContextMenu>
  );
}
