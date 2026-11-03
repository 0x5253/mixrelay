import { request as forwardRequest, type IncomingHttpHeaders } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import { type HtmlTagDescriptor, type Plugin, runnerImport } from 'vite';
import { configDefaults, defineConfig } from 'vitest/config';
import { engineSocket } from '../../scripts/local-socket.mjs';

/** Puts the token theme CSS and the saved-preference bootstrap at the start of <head>.
 * The tags come from src/core/theme/themeDocument.ts, loaded through Vite's module
 * runner so this config never imports application source directly. Only the app's
 * index.html is changed; Storybook's iframe document is left to its own Provider. */
function mixrelayThemeDocument(): Plugin {
  const module = fileURLToPath(new URL('./src/core/theme/themeDocument.ts', import.meta.url));
  const appDocument = fileURLToPath(new URL('./index.html', import.meta.url));
  return {
    name: 'mixrelay-theme-document',
    async transformIndexHtml(_html, context) {
      // Only the application document: Storybook's iframe keeps its controlled theme.
      if (path.resolve(context.filename) !== appDocument) return undefined;
      const { module: theme } = await runnerImport<{
        themeDocumentTags: () => HtmlTagDescriptor[];
      }>(module, { configFile: false, logLevel: 'silent' });
      return theme.themeDocumentTags();
    },
  };
}

// Connection-scoped headers belong to each hop, never forwarded (RFC 9110 §7.6.1).
const HOP_BY_HOP = new Set([
  'connection',
  'keep-alive',
  'proxy-authenticate',
  'proxy-authorization',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
]);

function endToEnd(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  const named = String(headers.connection ?? '')
    .split(',')
    .map((name) => name.trim().toLowerCase());
  return Object.fromEntries(
    Object.entries(headers).filter(([name]) => !HOP_BY_HOP.has(name) && !named.includes(name)),
  );
}

/** Development only, opt-in (`MIXRELAY_DEV_ENGINE_BRIDGE=1`): the browser cannot reach
 * the engine's Unix socket, so `vite dev` on localhost forwards same-origin `/engine/*`
 * requests to it. Requests must name this dev server as their Host (no DNS rebinding) and
 * carry no foreign Origin (no other site's page). While it runs, anything that can reach
 * the dev server, including other accounts on this Mac, acts with the developer's access
 * to the engine; it is never part of a build. */
function mixrelayEngineBridge(): Plugin {
  return {
    name: 'mixrelay-engine-bridge',
    apply: 'serve',
    configureServer(server) {
      if (process.env.MIXRELAY_DEV_ENGINE_BRIDGE !== '1') return;
      const { host, port } = server.config.server;
      if (typeof host !== 'string' || !['127.0.0.1', 'localhost', '::1'].includes(host)) {
        server.config.logger.warn('engine bridge disabled: the dev server is not loopback-only');
        return;
      }
      const hosts = [`127.0.0.1:${port}`, `localhost:${port}`, `[::1]:${port}`];
      server.middlewares.use('/engine', (incoming, outgoing) => {
        const origin = incoming.headers.origin;
        if (
          !hosts.includes(incoming.headers.host ?? '') ||
          (origin !== undefined && !hosts.some((allowed) => origin === `http://${allowed}`))
        ) {
          outgoing.statusCode = 403;
          outgoing.end('engine bridge accepts only this dev server');
          return;
        }
        const {
          host: _host,
          origin: _origin,
          referer: _referer,
          cookie: _cookie,
          ...headers
        } = endToEnd(incoming.headers);
        const forward = forwardRequest(
          {
            socketPath: engineSocket(),
            method: incoming.method,
            path: incoming.url,
            headers: { ...headers, host: 'mixrelay-engine' },
          },
          (response) => {
            outgoing.writeHead(response.statusCode ?? 502, endToEnd(response.headers));
            // An engine that drops a streamed response ends the browser's too.
            response.on('aborted', () => outgoing.destroy());
            response.on('error', () => outgoing.destroy());
            response.pipe(outgoing);
          },
        );
        forward.on('error', () => {
          if (outgoing.headersSent) return outgoing.destroy();
          outgoing.statusCode = 503;
          outgoing.end('engine unavailable');
        });
        // A browser that goes away releases the engine request (and any stream) too.
        outgoing.on('close', () => {
          if (!outgoing.writableFinished) forward.destroy();
        });
        incoming.pipe(forward);
      });
    },
  };
}

const tauriHost = process.env.TAURI_DEV_HOST;
// Story accessibility rendering is heavy; it runs as its own step so it never competes
// with timing-sensitive behaviour tests (pnpm test:a11y).
const a11yRun = process.env.MIXRELAY_UI_A11Y === '1';
const a11yTests = 'src/**/*.a11y.test.tsx';

export default defineConfig({
  plugins: [react(), mixrelayThemeDocument(), mixrelayEngineBridge()],
  clearScreen: false,
  server: {
    host: tauriHost || '127.0.0.1',
    port: 1420,
    strictPort: true,
  },
  test: {
    maxWorkers: Number(process.env.MIXRELAY_UI_TEST_WORKERS ?? 4),
    // `.test.tsx` renders or touches the DOM and runs in jsdom; `.test.ts` is DOM-free and
    // runs in Node without jsdom's per-file environment cost. A logic test that needs the
    // DOM is named `.test.tsx`.
    projects: [
      {
        extends: true as const,
        test: {
          name: 'dom',
          environment: 'jsdom',
          setupFiles: './src/test-setup.ts',
          include: a11yRun ? [a11yTests] : ['src/**/*.test.tsx'],
          exclude: [...configDefaults.exclude, ...(a11yRun ? [] : [a11yTests])],
        },
      },
      ...(a11yRun
        ? []
        : [
            {
              extends: true as const,
              test: {
                name: 'node',
                environment: 'node',
                setupFiles: './src/test-setup.node.ts',
                include: ['src/**/*.test.ts'],
              },
            },
          ]),
    ],
    coverage: {
      reportsDirectory: process.env.MIXRELAY_UI_COVERAGE_DIR ?? './coverage',
      exclude: [
        // Storybook-only documentation is verified in its built browser surface.
        'src/core/design-system/**',
        'src/**/*.stories.{ts,tsx}',
        'src/**/*.test.{ts,tsx}',
        'src/main.tsx',
        'src/test-setup.ts',
        'src/test-setup.node.ts',
      ],
      include: ['src/**/*.{ts,tsx}'],
      provider: 'v8',
      reporter: ['text', 'json'],
      thresholds: {
        branches: 100,
        functions: 100,
        lines: 100,
        statements: 100,
      },
    },
  },
});
