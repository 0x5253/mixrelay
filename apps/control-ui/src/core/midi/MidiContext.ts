import { createContext, type ReactNode } from 'react';
import type { MidiAssignment, MidiBinding, MidiStatus } from '../domain/midi';

export const MidiContext = createContext<{
  active: boolean;
  status: MidiStatus | null;
  selected: MidiAssignment | null;
  toggle: () => void;
  assign: (assignment: MidiAssignment) => void;
  remove: (binding: MidiBinding) => void;
} | null>(null);
export const MidiPanelContext = createContext<ReactNode>(null);
export const MidiGraphContext = createContext<{ graphId: string; graphName: string } | null>(null);
export const MidiNodeContext = createContext<{ nodeId: string; nodeName: string } | null>(null);
