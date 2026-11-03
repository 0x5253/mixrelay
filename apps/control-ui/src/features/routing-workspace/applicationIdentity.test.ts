import { describe, expect, it } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type {
  CapturableApplication,
  RuntimeApplicationSelector,
} from '../../core/domain/control/inventory';
import { applicationInventorySelection, applicationSelectorKey } from './applicationIdentity';
import { presentRoutingApplicationIcons } from './model';

function application(
  selector: RuntimeApplicationSelector = {
    match: 'bundle_id',
    bundle_id: 'org.example.player',
    include_descendants: false,
  },
  processId = 10,
): CapturableApplication {
  return {
    bundleId: 'org.example.player',
    displayName: 'Player',
    processId,
    selector,
    iconDataUrl: `data:image/png;base64,pid-${processId}`,
  };
}
function graph(selector: unknown): AudioGraphModel {
  return {
    revision: 7,
    edges: [],
    nodes: [
      {
        id: 'source',
        position: { x: 12, y: 24 },
        inputPorts: [],
        outputPorts: [],
        appearance: { title: 'My saved label', category: 'Application', icon: 'APP' },
        runtime: { type: 'application_capture', config: { selector, gainDb: -6 } },
      },
    ],
  };
}
function process(processId: number): RuntimeApplicationSelector {
  return { match: 'process_id', process_id: processId, include_descendants: true };
}

describe('application presentation identity across platforms', () => {
  it('keeps menu choices distinct for instances sharing an application ID', () => {
    const first = application(process(10)),
      second = application(process(20), 20);
    expect(applicationInventorySelection(first)?.id).not.toBe(
      applicationInventorySelection(second)?.id,
    );
    expect(applicationInventorySelection({ ...first, displayName: 'Translated name' })?.id).toBe(
      applicationInventorySelection(first)?.id,
    );
    expect(
      [first, second].find(
        (item) =>
          applicationInventorySelection(item)?.id === applicationInventorySelection(second)?.id,
      ),
    ).toBe(second);
  });

  it('rejects absent, malformed and process-mismatched inventory selectors', () => {
    const valid = application(process(10));
    expect(
      applicationInventorySelection({
        ...valid,
        selector: undefined,
      } as unknown as CapturableApplication),
    ).toBeUndefined();
    expect(
      applicationInventorySelection({
        ...valid,
        selector: { match: 'process_id', process_id: 0, include_descendants: true },
      }),
    ).toBeUndefined();
    expect(applicationInventorySelection({ ...valid, processId: 20 })).toBeUndefined();
    expect(
      applicationInventorySelection({ ...application(), bundleId: 'org.example.other' }),
    ).toBeUndefined();
    expect(
      applicationInventorySelection({
        ...valid,
        selector: { match: 'all_except', bundle_ids: ['org.example.other'] },
      }),
    ).toBeUndefined();
  });

  it('matches a PID-selected node to that exact instance without changing capture config', () => {
    const original = graph(process(20));
    const shown = presentRoutingApplicationIcons(original, [
      application(process(10)),
      application(process(20), 20),
    ]);
    expect(shown.nodes[0]?.appearance.iconUrl).toBe('data:image/png;base64,pid-20');
    expect(shown.nodes[0]?.runtime).toBe(original.nodes[0]?.runtime);
    expect(shown.nodes[0]?.appearance.title).toBe('My saved label');
    expect(shown.revision).toBe(7);
  });

  it('preserves the current macOS explicit bundle selection', () => {
    const original = graph({
      match: 'bundle_id',
      bundle_id: 'org.example.player',
      include_descendants: true,
    });
    const shown = presentRoutingApplicationIcons(original, [application()]);
    expect(shown.nodes[0]?.appearance.iconUrl).toBe('data:image/png;base64,pid-10');
    expect(presentRoutingApplicationIcons(shown, [application()])).toBe(shown);
  });

  it('does not borrow a bundle icon from an inventory entry with a malformed explicit selector', () => {
    const selected = graph({
      match: 'bundle_id',
      bundle_id: 'org.example.player',
      include_descendants: true,
    });
    const malformed = {
      ...application(),
      selector: { match: 'process_id', process_id: 0, include_descendants: true },
    } as CapturableApplication;
    expect(
      presentRoutingApplicationIcons(selected, [malformed]).nodes[0]?.appearance.iconUrl,
    ).toBeUndefined();
  });

  it('retains an available icon when another instance of the same bundle has none', () => {
    const selected = graph({
      match: 'bundle_id',
      bundle_id: 'org.example.player',
      include_descendants: true,
    });
    const shown = presentRoutingApplicationIcons(selected, [
      application(),
      { ...application(undefined, 20), iconDataUrl: null },
    ]);
    expect(shown.nodes[0]?.appearance.iconUrl).toBe('data:image/png;base64,pid-10');
    expect(shown.nodes[0]?.appearance.title).toBe('My saved label');
  });
});

describe('application icon lifetime and executable identity', () => {
  it('matches explicit executable selectors by path, never filename alone', () => {
    const selected: RuntimeApplicationSelector = { match: 'executable_path', path: '/opt/player' };
    const shown = presentRoutingApplicationIcons(graph(selected), [
      application({ match: 'executable_path', path: '/other/player' }),
      application(selected, 20),
    ]);
    expect(shown.nodes[0]?.appearance.iconUrl).toBe('data:image/png;base64,pid-20');
  });

  it('clears an old icon on disappearance rather than guessing a new PID from its name', () => {
    const shown = presentRoutingApplicationIcons(graph(process(10)), [application(process(10))]);
    const gone = presentRoutingApplicationIcons(shown, [application(process(20), 20)]);
    expect(gone.nodes[0]?.appearance.iconUrl).toBeUndefined();
    expect(gone.nodes[0]?.runtime).toBe(shown.nodes[0]?.runtime);
    expect(presentRoutingApplicationIcons(shown, []).nodes[0]?.appearance.iconUrl).toBeUndefined();
    expect(
      presentRoutingApplicationIcons(shown, [{ ...application(process(10)), iconDataUrl: null }])
        .nodes[0]?.appearance.iconUrl,
    ).toBeUndefined();
    expect(
      presentRoutingApplicationIcons(shown, [{ ...application(process(10)), iconDataUrl: '' }])
        .nodes[0]?.appearance.iconUrl,
    ).toBeUndefined();
  });

  it('does not borrow a reused PID icon while a restored capture requires reselection', () => {
    const original = graph(process(10));
    const restored = {
      ...original,
      nodes: original.nodes.map((node) => {
        if (!node.runtime) throw new Error('Application fixture requires runtime configuration');
        return {
          ...node,
          runtime: {
            ...node.runtime,
            config: { ...(node.runtime.config as object), requiresReselection: true },
          },
        };
      }),
    };
    const shown = presentRoutingApplicationIcons(restored, [
      { ...application(process(10)), displayName: 'Different app' },
    ]);
    expect(shown.nodes[0]?.appearance.iconUrl).toBeUndefined();
    expect(shown.nodes[0]?.appearance.title).toBe('My saved label');
    expect(shown.nodes[0]?.runtime).toBe(restored.nodes[0]?.runtime);
    expect(shown.revision).toBe(restored.revision);
  });

  it('never invents one application icon for all-except or malformed selectors', () => {
    for (const selector of [
      null,
      {},
      { match: 'process_id', process_id: '10' },
      { match: 'process_id', process_id: -1 },
      { match: 'process_id', process_id: 10, include_descendants: 'yes' },
      { match: 'bundle_id', bundle_id: '' },
      { match: 'bundle_id', bundle_id: 'org.example.player' },
      { match: 'all_except', bundle_ids: ['org.example.player'] },
    ]) {
      expect(applicationSelectorKey(selector)).toBeUndefined();
      expect(
        presentRoutingApplicationIcons(graph(selector), [application()]).nodes[0]?.appearance
          .iconUrl,
      ).toBeUndefined();
    }
  });
});
