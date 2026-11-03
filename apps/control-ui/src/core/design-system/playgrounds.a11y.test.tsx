import { composeStories, setProjectAnnotations } from '@storybook/react-vite';
import { act, render } from '@testing-library/react';
import axe from 'axe-core';
import type { ComponentType } from 'react';
import { createPortal } from 'react-dom';
import { describe, expect, it } from 'vitest';
import preview from '../../../.storybook/preview';

/** Storybook's a11y parameter is enforced here for every public Core playground state.
 * The whole body is scanned so portalled dialogs, menus and tooltips are included.
 * jsdom cannot compute colour contrast and isolated components have no page landmarks,
 * so those two rules belong to the browser review; every other axe rule applies.
 * A story opts out only through Storybook's own `parameters.a11y.test = 'off'`. */
setProjectAnnotations([preview]);
type StoryModule = Parameters<typeof composeStories>[0];
type PortableStory = ComponentType & {
  load: () => Promise<void>;
  parameters?: { a11y?: { test?: string } };
};
const modules = import.meta.glob<StoryModule>('./playgrounds/*.stories.tsx', { eager: true });
const disabledRules = { 'color-contrast': { enabled: false }, region: { enabled: false } };

it('uses an axe configuration that still reports real violations', async () => {
  const view = render(
    <div>
      <button type="button" />
      {/* biome-ignore lint/a11y/useFocusableInteractive: deliberately invalid, so axe must report it. */}
      <span role="slider" aria-valuenow={1} />
    </div>,
  );
  const results = await axe.run(document.body, { rules: disabledRules });
  expect(results.violations.map((violation) => violation.id).sort()).toEqual(
    expect.arrayContaining(['aria-input-field-name', 'button-name']),
  );
  view.unmount();
});

it('inspects content portalled outside the rendered story', async () => {
  const view = render(<div>{createPortal(<button type="button" />, document.body)}</div>);
  const results = await axe.run(document.body, { rules: disabledRules });
  expect(results.violations.map((violation) => violation.id)).toContain('button-name');
  view.unmount();
});

it('renders every playground module', () => {
  expect(Object.keys(modules).length).toBeGreaterThanOrEqual(60);
});

for (const [file, module] of Object.entries(modules)) {
  const stories = composeStories(module) as unknown as Record<string, PortableStory>;
  describe(file.replace('./playgrounds/', ''), () => {
    for (const [name, Story] of Object.entries(stories)) {
      const opted = Story.parameters?.a11y?.test === 'off';
      it.skipIf(opted)(`${name} has no axe violations`, async () => {
        await act(async () => {
          await Story.load();
        });
        const view = render(<Story />);
        await act(async () => {});
        const results = await axe.run(document.body, { rules: disabledRules });
        expect(
          results.violations.map((violation) => ({
            rule: violation.id,
            nodes: violation.nodes.map((node) => node.html.slice(0, 160)),
          })),
        ).toEqual([]);
        view.unmount();
      });
    }
  });
}
