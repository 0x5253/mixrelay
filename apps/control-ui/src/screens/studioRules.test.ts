import { describe, expect, it } from 'vitest';
import { i18n } from '../core/i18n';
import {
  catalogErrorText,
  diagnosticsAnnouncement,
  graphChoiceOutcome,
  graphListHistoryAction,
  isNewGraphShortcut,
} from './studioRules';

const key = (key: string, modifiers: { altKey?: boolean; shiftKey?: boolean } = {}) => ({
  key,
  altKey: false,
  shiftKey: false,
  ...modifiers,
});

describe('isNewGraphShortcut', () => {
  const press = { ...key('N', { shiftKey: true }), defaultPrevented: false };

  it('opens a graph with the platform modifier, Shift and N outside text entry', () => {
    expect(isNewGraphShortcut(press, true, false)).toBe(true);
    expect(isNewGraphShortcut({ ...press, key: 'n' }, true, false)).toBe(true);
  });

  it('preserves text editing, other modifiers and keys another control handled', () => {
    expect(isNewGraphShortcut(press, true, true)).toBe(false);
    expect(isNewGraphShortcut(press, false, false)).toBe(false);
    expect(isNewGraphShortcut({ ...press, shiftKey: false }, true, false)).toBe(false);
    expect(isNewGraphShortcut({ ...press, altKey: true }, true, false)).toBe(false);
    expect(isNewGraphShortcut({ ...press, defaultPrevented: true }, true, false)).toBe(false);
    expect(isNewGraphShortcut({ ...press, key: 'M' }, true, false)).toBe(false);
  });
});

describe('graphListHistoryAction', () => {
  it('undoes a graph deletion with the platform modifier and Z, and redoes it with Shift', () => {
    expect(graphListHistoryAction(key('z'), true, false)).toBe('undo_delete');
    expect(graphListHistoryAction(key('Z', { shiftKey: true }), true, false)).toBe('redo_delete');
  });

  it('leaves text entry, other modifiers and other keys alone', () => {
    expect(graphListHistoryAction(key('z'), true, true)).toBeNull();
    expect(graphListHistoryAction(key('z'), false, false)).toBeNull();
    expect(graphListHistoryAction(key('z', { altKey: true }), true, false)).toBeNull();
    expect(graphListHistoryAction(key('y'), true, false)).toBeNull();
  });
});

describe('graphChoiceOutcome', () => {
  it('closes the overlay only once the chosen graph is active', () => {
    expect(graphChoiceOutcome('b', { activeId: 'b', busy: true, error: null })).toBe('activated');
    expect(graphChoiceOutcome('b', { activeId: 'a', busy: true, error: null })).toBe('pending');
  });

  it('abandons a choice that settles without activating it, so a later refresh cannot reuse it', () => {
    expect(graphChoiceOutcome('b', { activeId: 'a', busy: false, error: null })).toBe('abandoned');
    expect(graphChoiceOutcome('b', { activeId: 'a', busy: true, error: 'graph_invalid' })).toBe(
      'abandoned',
    );
    expect(graphChoiceOutcome('b', { activeId: undefined, busy: false, error: null })).toBe(
      'abandoned',
    );
  });
});

it('describes catalog failures with the specific or standard recovery text', () => {
  const t = i18n.t.bind(i18n);
  expect(catalogErrorText(t, null)).toBeNull();
  expect(catalogErrorText(t, 'graph_catalog_unavailable')).toBe(t('studio.catalogUnavailable'));
  expect(catalogErrorText(t, 'graph_invalid')).toBe(t('routing.errors.graph_invalid'));
  expect(catalogErrorText(t, 'unknown_code')).toBe(t('routing.errors.control_request_failed'));
});

it('announces the diagnostics count, including a change back to zero', () => {
  const t = i18n.t.bind(i18n);
  expect(diagnosticsAnnouncement(t, 2)).toBe(t('studio.diagnosticsAnnouncement', { count: 2 }));
  expect(diagnosticsAnnouncement(t, 0)).toBe(t('studio.diagnosticsClear'));
});
