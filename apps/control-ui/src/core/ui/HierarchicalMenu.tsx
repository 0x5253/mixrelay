import {
  cloneElement,
  type MouseEventHandler,
  type PropsWithChildren,
  type ReactElement,
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from 'react';
import {
  Button,
  Header,
  Menu,
  MenuItem,
  MenuSection,
  MenuTrigger,
  Popover,
  Separator,
  SubmenuTrigger,
  Text,
} from 'react-aria-components';
import { useTranslation } from 'react-i18next';
import styles from './HierarchicalMenu.module.css';
import { Tooltip } from './Tooltip';

export interface HierarchicalMenuItem {
  children?: readonly HierarchicalMenuItem[];
  description?: string;
  disabled?: boolean;
  destructive?: boolean;
  id: string;
  iconUrl?: string;
  kind?: 'divider' | 'item' | 'label';
  label: string;
}
interface MenuContentProps {
  items: readonly HierarchicalMenuItem[];
  onSelect: (itemId: string) => void;
}

export function HierarchicalMenuButton({
  buttonClassName,
  buttonContent,
  label,
  items,
  onOpenedChange,
  onSelect,
  opened,
  tooltip,
}: MenuContentProps & {
  buttonClassName?: string;
  buttonContent: ReactNode;
  label: string;
  onOpenedChange?: (opened: boolean) => void;
  opened?: boolean;
  tooltip?: string;
}) {
  const button = (
    <Button aria-label={label} className={`${styles.trigger} ${buttonClassName ?? ''}`}>
      {buttonContent}
    </Button>
  );
  return (
    <MenuTrigger isOpen={opened} onOpenChange={onOpenedChange}>
      {tooltip ? (
        <Tooltip label={tooltip} position="right">
          {button}
        </Tooltip>
      ) : (
        button
      )}
      <Popover
        style={{ zIndex: 'var(--layer-floating)' }}
        className={styles.dropdown}
        placement="bottom start"
        offset={4}
      >
        <Menu className={styles.scrollArea} aria-label={label}>
          {menuEntries(items, onSelect)}
        </Menu>
      </Popover>
    </MenuTrigger>
  );
}

/** A DOM point anchors React Aria's positioning; no separate menu keyboard implementation. */
export function HierarchicalContextMenu({
  children,
  disabled,
  items,
  getAnchor,
  onOpenedChange,
  onSelect,
  opened,
}: PropsWithChildren<
  MenuContentProps & {
    disabled?: boolean;
    getAnchor?: (event: React.MouseEvent) => { x: number; y: number };
    onOpenedChange?: (opened: boolean) => void;
    opened?: boolean;
  }
>) {
  const { t } = useTranslation();
  const [internalOpen, setInternalOpen] = useState(false);
  const [point, setPoint] = useState({ x: 0, y: 0 });
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    if (disabled) clearTimeout(timer.current);
    return () => clearTimeout(timer.current);
  }, [disabled]);
  const changeOpen = (value: boolean) => {
    setInternalOpen(value);
    onOpenedChange?.(value);
  };
  const open = (x: number, y: number) => {
    if (!disabled) {
      setPoint({ x, y });
      changeOpen(true);
    }
  };
  const child = children as ReactElement<{
    onContextMenu?: MouseEventHandler;
    onTouchStart?: React.TouchEventHandler;
    onTouchEnd?: React.TouchEventHandler;
    onTouchMove?: React.TouchEventHandler;
  }>;
  return (
    <>
      {cloneElement(child, {
        onContextMenu: (event) => {
          child.props.onContextMenu?.(event);
          if (disabled) return;
          event.preventDefault();
          const point = getAnchor?.(event) ?? { x: event.clientX, y: event.clientY };
          open(point.x, point.y);
        },
        onTouchStart: (event) => {
          child.props.onTouchStart?.(event);
          const touch = event.touches[0];
          if (touch) timer.current = setTimeout(() => open(touch.clientX, touch.clientY), 500);
        },
        onTouchEnd: (event) => {
          clearTimeout(timer.current);
          child.props.onTouchEnd?.(event);
        },
        onTouchMove: (event) => {
          clearTimeout(timer.current);
          child.props.onTouchMove?.(event);
        },
      })}
      <MenuTrigger isOpen={opened ?? internalOpen} onOpenChange={changeOpen}>
        <Button
          aria-label={t('common.actions')}
          render={(props) => <button {...props} tabIndex={-1} aria-hidden="true" />}
          excludeFromTabOrder
          className={styles.cursorAnchor}
          style={{ left: point.x, top: point.y }}
        />
        <Popover
          style={{ zIndex: 'var(--layer-floating)' }}
          className={styles.dropdown}
          placement="bottom start"
          offset={2}
          isNonModal
        >
          <Menu className={styles.scrollArea} aria-label={t('common.actions')}>
            {menuEntries(items, (id) => {
              onSelect(id);
              changeOpen(false);
            })}
          </Menu>
        </Popover>
      </MenuTrigger>
    </>
  );
}

function menuEntries(
  items: readonly HierarchicalMenuItem[],
  onSelect: MenuContentProps['onSelect'],
): ReactNode[] {
  const entries: ReactNode[] = [];
  for (let index = 0; index < items.length; index++) {
    // The bounded index reads the dense item collection supplied by the owner.
    const item = items[index] as HierarchicalMenuItem;
    if (item.kind === 'divider')
      entries.push(<Separator className={styles.divider} key={item.id} />);
    else if (item.kind === 'label') {
      const group: HierarchicalMenuItem[] = [];
      let next = items[index + 1];
      while (isGroupEntry(next)) {
        group.push(next);
        next = items[++index + 1];
      }
      entries.push(
        <MenuSection key={item.id}>
          <Header className={styles.label}>{item.label}</Header>
          {menuEntries(group, onSelect)}
        </MenuSection>,
      );
    } else entries.push(menuEntry(item, onSelect));
  }
  return entries;
}

function menuEntry(
  item: HierarchicalMenuItem,
  onSelect: MenuContentProps['onSelect'],
): ReactElement {
  const children = item.children ?? [];
  const hasChildren = children.length > 0;
  const entry = (
    <MenuItem
      id={item.id}
      textValue={item.label}
      isDisabled={item.disabled}
      className={styles.item}
      data-description={item.description || undefined}
      data-destructive={item.destructive || undefined}
      onAction={hasChildren ? undefined : () => onSelect(item.id)}
    >
      {item.iconUrl ? (
        <img alt="" className={styles.itemIcon} src={item.iconUrl} width={18} height={18} />
      ) : null}
      <span className={styles.itemText}>
        <Text slot="label" className={styles.itemTitle}>
          {item.label}
        </Text>
        {item.description ? (
          <Text slot="description" className={styles.itemDescription}>
            {item.description}
          </Text>
        ) : null}
      </span>
      {hasChildren ? <span aria-hidden="true">›</span> : null}
    </MenuItem>
  );
  return hasChildren ? (
    <SubmenuTrigger key={item.id}>
      {entry}
      <Popover
        style={{ zIndex: 'var(--layer-floating)' }}
        className={`${styles.dropdown} ${styles.submenu}`}
        placement="right top"
        offset={3}
      >
        <Menu className={styles.scrollArea} aria-label={item.label}>
          {menuEntries(children, onSelect)}
        </Menu>
      </Popover>
    </SubmenuTrigger>
  ) : (
    cloneElement(entry, { key: item.id })
  );
}

function isGroupEntry(item: HierarchicalMenuItem | undefined): item is HierarchicalMenuItem {
  return Boolean(item && item.kind !== 'label' && item.kind !== 'divider');
}
