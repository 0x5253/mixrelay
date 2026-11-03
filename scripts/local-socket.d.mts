// Types for scripts/local-socket.mjs, used by TypeScript configuration (Vite dev bridge).
export const ENGINE_SOCKET: string;
export const DRIVER_SOCKET: string;
export function socketDirectory(
  environment?: NodeJS.ProcessEnv,
  platform?: NodeJS.Platform,
): string;
export function engineSocket(environment?: NodeJS.ProcessEnv, platform?: NodeJS.Platform): string;
export function driverSocket(environment?: NodeJS.ProcessEnv, platform?: NodeJS.Platform): string;
export interface SocketResponse {
  status: number | undefined;
  headers: import('node:http').IncomingHttpHeaders;
  body: string;
}
export function socketRequest(
  socketPath: string,
  path: string,
  options?: {
    method?: string;
    headers?: Record<string, string>;
    body?: string;
    timeoutMs?: number;
    responseLimit?: number;
  },
): Promise<SocketResponse>;
export function socketJson(
  socketPath: string,
  path: string,
  options?: {
    method?: string;
    headers?: Record<string, string>;
    body?: string;
    json?: unknown;
    timeoutMs?: number;
    responseLimit?: number;
  },
): Promise<unknown>;
