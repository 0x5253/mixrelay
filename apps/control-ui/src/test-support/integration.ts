import { vi } from 'vitest';

/** Whole-screen integration tests render App, StudioScreen or RoutingWorkspace with
 * their ports. Under CPU contention one of them can exceed the 5 s unit-test default,
 * so a file that renders a screen imports this module once. The timeout only guards
 * against hangs; speed is measured separately, and Testing Library's own waitFor/findBy
 * deadlines are unchanged. The boundary check requires this import in such files. */
export const integrationTestTimeout = 15_000;

vi.setConfig({ testTimeout: integrationTestTimeout });
