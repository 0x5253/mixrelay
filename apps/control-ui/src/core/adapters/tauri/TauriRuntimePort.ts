import { Channel } from '@tauri-apps/api/core';
import {
  isRuntimeConnectStage,
  type RuntimeConnectStage,
  type RuntimeStatus,
} from '../../domain/runtime';
import type { RuntimePort } from '../../ports/RuntimePort';
import { literal } from '../decode';
import { type NativeInvoke, tauriInvoke } from './nativeInvoke';

const runtimeStatus = literal('ready', 'not_installed', 'incompatible', 'unavailable');

/** A native progress channel delivering untyped messages. */
type StageChannel = (onMessage: (message: unknown) => void) => unknown;

const tauriChannel: StageChannel = (onMessage) => new Channel<unknown>(onMessage);

export class TauriRuntimePort implements RuntimePort {
  constructor(
    private readonly invoke: NativeInvoke = tauriInvoke,
    private readonly channel: StageChannel = tauriChannel,
  ) {}

  async connect(onStage: (stage: RuntimeConnectStage) => void): Promise<RuntimeStatus> {
    // Stages are progress only; an unknown message never changes the connection outcome.
    const stages = this.channel((message) => {
      if (isRuntimeConnectStage(message)) onStage(message);
    });
    return runtimeStatus(
      await this.invoke('runtime_connect', { onStage: stages }),
      'runtime_connect',
    );
  }

  async status(): Promise<RuntimeStatus> {
    return runtimeStatus(await this.invoke('runtime_status'), 'runtime_status');
  }
}
