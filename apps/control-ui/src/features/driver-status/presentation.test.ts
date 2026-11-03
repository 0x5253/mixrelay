import { describe, expect, it } from 'vitest';
import { driverPresentation, getDriverPresentation } from '../../core/driver/presentation';
import { en } from '../../core/i18n/locales/en';
import { koKR } from '../../core/i18n/locales/ko-KR';
import { pseudo } from '../../core/i18n/locales/pseudo';

describe('shared driver presentation', () => {
  it('shows unknown future states as unavailable instead of crashing or claiming readiness', () => {
    for (const state of ['future_state', 'toString', '__proto__']) {
      expect(getDriverPresentation(state)).toBe(driverPresentation.unavailable);
    }
    expect(getDriverPresentation('ready')).toBe(driverPresentation.ready);
  });
  it('distinguishes transport failure from installation and permission failures', () => {
    expect(driverPresentation.unavailable).toEqual({
      action: 'actionDiagnose',
      label: 'unavailable',
      tone: 'critical',
    });
    expect(driverPresentation.blocked.label).not.toBe(driverPresentation.unavailable.label);
    expect(driverPresentation.not_installed.action).toBe('actionInstall');
    expect(driverPresentation.ready.tone).toBe('positive');
  });

  it('supplies every state in every locale without naming an unrelated OS', () => {
    for (const locale of [en, koKR, pseudo]) {
      const labels: Record<string, string> = locale.driver.states;
      for (const presentation of Object.values(driverPresentation)) {
        expect(labels[presentation.label]).toBeTruthy();
      }
      expect(labels.blocked).not.toMatch(/macOS|måcÖS/i);
      expect(labels.unavailable).not.toBe(labels.notInstalled);
    }
  });
});
