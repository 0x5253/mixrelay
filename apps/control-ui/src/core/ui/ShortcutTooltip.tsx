import type { ReactElement } from 'react';
import { formatPlatformShortcut } from './platformShortcut';
import { Tooltip } from './Tooltip';

export interface ShortcutTooltipProps {
  children: ReactElement;
  shortcut: string;
}

export function ShortcutTooltip({ children, shortcut }: ShortcutTooltipProps) {
  return (
    <Tooltip label={formatPlatformShortcut(shortcut)} position="right">
      {children}
    </Tooltip>
  );
}
