import { describe, expect, it } from 'vitest';
import {
  brandColors,
  logoColor,
  meterScale,
  mixrelayVariables,
  palette,
  sizes,
  spacing,
  themeRoles,
} from './tokens';

function luminance(hex: string) {
  const rgb = [1, 3, 5].map((offset) => {
    const value = Number.parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return (rgb[0] ?? 0) * 0.2126 + (rgb[1] ?? 0) * 0.7152 + (rgb[2] ?? 0) * 0.0722;
}

function contrast(a: string, b: string) {
  const values = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((values[0] ?? 0) + 0.05) / ((values[1] ?? 0) + 0.05);
}

it('keeps all DSUB steps in descending luminance and the brand source fixed', () => {
  for (const steps of Object.values(palette)) {
    expect(Object.keys(steps)).toEqual([
      '50',
      '100',
      '200',
      '300',
      '400',
      '500',
      '600',
      '700',
      '800',
      '900',
    ]);
    const values = Object.values(steps).map(luminance);
    expect(values).toEqual([...values].sort((a, b) => b - a));
  }
  expect(logoColor).toBe('#b02d23');
  // Brand assets are approved artwork, independent of the UI palette.
  expect(brandColors).toEqual({
    primary: '#b02d23',
    ink: '#242622',
    inverse: '#edeee7',
    paper: '#faf9f5',
    dark: '#161918',
    muted: '#5c594f',
    white: '#ffffff',
  });
  expect(mixrelayVariables['--logo-source']).toBe(logoColor);
  expect(Object.keys(themeRoles.light)).toEqual(Object.keys(themeRoles.dark));
  expect(Object.keys(themeRoles.light).some((role) => role.includes('orange'))).toBe(false);
});

it('keeps spacing and size roles on the shared scale', () => {
  const steps = new Set<number>(Object.values(spacing));
  expect(Object.values(sizes).every((size) => size % spacing[1] === 0)).toBe(true);
  for (const [name, value] of Object.entries(mixrelayVariables)) {
    if (name.startsWith('--space-')) expect(steps.has(Number.parseFloat(value)), name).toBe(true);
    // Insets, gaps and paddings name a step or role; a raw length here bypasses the scale.
    else if (/-(?:padding|inset|gap|margin)(?:-|$)/u.test(name))
      expect(value, name).not.toMatch(/\d+px/u);
  }
});

it('places meter zones at -12 and -3 dBFS on the -60 dBFS scale', () => {
  expect(meterScale).toEqual({ floorDb: -60, highPercent: 80, peakPercent: 95 });
  expect(mixrelayVariables['--meter-high-start']).toBe('80%');
  expect(mixrelayVariables['--meter-peak-start']).toBe('95%');
});

it('keeps field placeholders on a text role whose contrast is checked below', () => {
  // Placeholders show the original name in rename fields, so they are information.
  expect(mixrelayVariables['--control-placeholder']).toBe('var(--text-muted)');
});

describe.each(['light', 'dark'] as const)('%s theme contrast', (scheme) => {
  const roles = themeRoles[scheme];
  const surfaces = [
    '--background',
    '--surface',
    '--surface-muted',
    '--surface-hover',
    '--surface-active',
  ] as const;
  it('keeps normal and semantic text readable on default, hover and pressed surfaces', () => {
    const textRoles = [
      '--text-primary',
      '--text-secondary',
      '--text-muted',
      '--status-accent',
      '--status-positive',
      '--status-warning',
      '--status-danger',
    ] as const;
    for (const surface of surfaces) {
      for (const text of textRoles)
        expect(
          contrast(roles[text], roles[surface]),
          `${text} on ${surface}`,
        ).toBeGreaterThanOrEqual(4.5);
    }
  });
  it('keeps fields, selection and keyboard focus distinguishable', () => {
    for (const surface of surfaces) {
      for (const boundary of [
        '--border-control',
        '--text-primary',
        '--text-secondary',
        '--status-danger',
      ] as const) {
        expect(
          contrast(roles[boundary], roles[surface]),
          `${boundary} on ${surface}`,
        ).toBeGreaterThanOrEqual(3);
      }
    }
  });
  it('separates the neutral dial value from its dim track and every consumer surface', () => {
    expect(mixrelayVariables['--knob-indicator']).toBe('var(--text-primary)');
    const indicator = roles['--text-primary'];
    expect(contrast(indicator, roles['--knob-track'])).toBeGreaterThanOrEqual(3);
    for (const surface of [...surfaces, '--surface-node', '--node-background-off'] as const)
      expect(contrast(indicator, roles[surface]), surface).toBeGreaterThanOrEqual(3);
  });
  it('raises content above the canvas in a visible hierarchy', () => {
    // Canvas < header/sidebar < node; hover is distinct from its node.
    expect(contrast(roles['--surface-node'], roles['--background'])).toBeGreaterThanOrEqual(1.3);
    expect(contrast(roles['--surface'], roles['--background'])).toBeGreaterThanOrEqual(1.1);
    expect(contrast(roles['--surface-hover'], roles['--surface-node'])).toBeGreaterThanOrEqual(
      1.12,
    );
    expect(contrast(roles['--node-background-off'], roles['--surface-node'])).toBeGreaterThan(1.03);
    // Node outlines and cables stay visible without outshining node faces.
    const cable = contrast(roles['--border-strong'], roles['--background']);
    expect(cable).toBeGreaterThanOrEqual(3);
    expect(cable).toBeLessThan(contrast(roles['--text-primary'], roles['--background']));
    expect(contrast(roles['--border-strong'], roles['--surface-node'])).toBeGreaterThanOrEqual(3);
  });
  it('keeps meter zones and recording distinguishable without relying on hue', () => {
    // Level meters and dynamics peak rails both draw the zones on the canvas tone.
    expect(mixrelayVariables['--meter-background']).toBe('var(--background)');
    expect(mixrelayVariables['--dynamics-rail-background']).toBe('var(--meter-background)');
    const track = roles['--background'];
    for (const zone of ['--meter-normal', '--meter-high', '--meter-peak'] as const)
      expect(contrast(roles[zone], track), zone).toBeGreaterThanOrEqual(3);
    expect(contrast(roles['--meter-normal'], roles['--meter-high'])).toBeGreaterThanOrEqual(1.25);
    expect(contrast(roles['--meter-high'], roles['--meter-peak'])).toBeGreaterThanOrEqual(1.25);
    for (const surface of ['--surface-node', '--surface-hover'] as const)
      expect(contrast(roles['--status-record'], roles[surface]), surface).toBeGreaterThanOrEqual(3);
  });
  it('keeps strong commands and semantic feedback readable', () => {
    for (const surface of [
      '--button-strong-background',
      '--button-strong-hover',
      '--button-strong-active',
    ] as const)
      expect(contrast(roles['--button-strong-text'], roles[surface])).toBeGreaterThanOrEqual(4.5);
    for (const tone of ['accent', 'positive', 'warning', 'danger'] as const) {
      expect(
        contrast(roles[`--status-${tone}`], roles[`--status-${tone}-surface`]),
      ).toBeGreaterThanOrEqual(4.5);
    }
  });
});
