import { renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { expect, it, vi } from 'vitest';
import type { GraphCatalog } from '../../core/domain/control/catalog';
import { MidiContext } from '../../core/midi/MidiContext';
import { midiAssignment } from './midiFixtures';
import { useMidiGraphSelection } from './useMidiGraphSelection';

const catalog: GraphCatalog = {
  protocolVersion: 1,
  catalogRevision: 1,
  activeId: 'other',
  graphs: [{ id: 'main', name: 'Programme' }],
  document: { revision: 1, schemaVersion: 2, nodes: [], edges: [] },
};
function provider(graphId: string) {
  return ({ children }: { children: ReactNode }) => (
    <MidiContext.Provider
      value={{
        active: true,
        status: null,
        selected: { ...midiAssignment, target: { graphId, nodeId: 'gain' } },
        toggle: vi.fn(),
        assign: vi.fn(),
        remove: vi.fn(),
      }}
    >
      {children}
    </MidiContext.Provider>
  );
}
it('navigates a selected mapping to its graph without editing it, including while mapping', () => {
  const mutate = vi.fn().mockResolvedValue(undefined);
  renderHook(
    () => useMidiGraphSelection({ catalog, busy: false, uncertain: false, error: null, mutate }),
    { wrapper: provider('main') },
  );
  expect(mutate).toHaveBeenCalledExactlyOnceWith({ action: 'select', id: 'main' });
});
it('leaves the current graph alone without a MIDI selection', () => {
  const mutate = vi.fn();
  renderHook(() =>
    useMidiGraphSelection({ catalog, busy: false, uncertain: false, error: null, mutate }),
  );
  expect(mutate).not.toHaveBeenCalled();
});
it.each([
  { catalog: null },
  { catalog: { ...catalog, activeId: 'main' } },
  { catalog: { ...catalog, graphs: [{ id: 'different', name: 'Unrelated' }] } },
  { busy: true },
  { uncertain: true },
  { error: 'offline' },
])('does not switch to a missing target or race an unavailable catalog: %j', (patch) => {
  const mutate = vi.fn();
  renderHook(
    () =>
      useMidiGraphSelection({
        catalog,
        busy: false,
        uncertain: false,
        error: null,
        mutate,
        ...patch,
      }),
    { wrapper: provider('main') },
  );
  expect(mutate).not.toHaveBeenCalled();
});
