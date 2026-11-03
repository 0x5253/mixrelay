import { ControlPortError } from '../core/domain/control/errors';
import type { ControlPort } from '../core/ports/control/ControlPort';

// Request bodies come from the adapter under test and reach the fake port unchanged; the
// casts only restate the argument the adapter encoded, which the contract then compares.
type Body = Record<string, unknown>;
type Call = (engine: Required<ControlPort>, params: string[], body: Body) => Promise<unknown>;

/** The engine's HTTP routes, served by a port so adapters can be tested against it. */
const routes: [method: string, path: RegExp, call: Call][] = [
  ['GET', /^\/api\/v1\/graph$/u, (engine) => engine.getGraph()],
  ['PUT', /^\/api\/v1\/graph$/u, (engine, _, body) => engine.applyGraph(body as never)],
  ['GET', /^\/api\/v1\/graph\/status$/u, (engine) => engine.getGraphStatus()],
  ['GET', /^\/api\/v1\/status$/u, (engine) => engine.getStatus()],
  ['GET', /^\/api\/v1\/meters$/u, (engine) => engine.getMeters()],
  ['GET', /^\/api\/v1\/graphs$/u, (engine) => engine.getGraphCatalog()],
  ['PUT', /^\/api\/v1\/graphs$/u, (engine, _, body) => engine.mutateGraphCatalog(body as never)],
  [
    'GET',
    /^\/api\/v1\/graphs\/([^/]+)\/layout$/u,
    (engine, [graphId]) => engine.getGraphLayout(graphId as string),
  ],
  [
    'PUT',
    /^\/api\/v1\/graphs\/([^/]+)\/layout$/u,
    (engine, [graphId], body) => engine.setGraphLayout(graphId as string, body as never),
  ],
  [
    'GET',
    /^\/api\/v1\/graphs\/([^/]+)\/nodes\/controls$/u,
    (engine, [graphId]) => engine.getNodeControls(graphId as string),
  ],
  [
    'PUT',
    /^\/api\/v1\/graphs\/([^/]+)\/nodes\/([^/]+)\/control$/u,
    (engine, [graphId, nodeId], body) =>
      engine.updateNodeControl(graphId as string, nodeId as string, body as never),
  ],
  [
    'GET',
    /^\/api\/v1\/graphs\/([^/]+)\/recorders\/([^/]+)$/u,
    (engine, [graphId, nodeId]) => engine.recorderStatus(graphId as string, nodeId as string),
  ],
  [
    'PUT',
    /^\/api\/v1\/graphs\/([^/]+)\/recorders\/([^/]+)$/u,
    (engine, [graphId, nodeId], body) =>
      engine.controlRecorder(
        graphId as string,
        nodeId as string,
        body.request as never,
        body.expectedRevision as number,
      ),
  ],
  ['GET', /^\/api\/v1\/processors$/u, (engine) => engine.getProcessors()],
  [
    'PUT',
    /^\/api\/v1\/processors\/scan$/u,
    (engine, _, body) => engine.rescanProcessors(body.paths as string[]),
  ],
  [
    'PUT',
    /^\/api\/v1\/processors\/retry$/u,
    (engine, _, body) => engine.retryProcessor(body as never),
  ],
  [
    'GET',
    /^\/api\/v1\/processors\/([^/]+)\/editor$/u,
    (engine, [nodeId]) => engine.getProcessorEditor(nodeId as string),
  ],
  [
    'PUT',
    /^\/api\/v1\/processors\/([^/]+)\/editor$/u,
    (engine, [nodeId], body) =>
      engine.setProcessorEditor(
        nodeId as string,
        body.open as boolean,
        body.expectedRevision as number,
      ),
  ],
  [
    'GET',
    /^\/api\/v1\/processors\/([^/]+)\/editor\/frame$/u,
    (engine, [nodeId]) => engine.getProcessorEditorFrame(nodeId as string),
  ],
  [
    'POST',
    /^\/api\/v1\/processors\/([^/]+)\/editor\/input$/u,
    (engine, [nodeId], body) => engine.sendProcessorEditorInput(nodeId as string, body as never),
  ],
  [
    'PUT',
    /^\/api\/v1\/processors\/([^/]+)\/parameters\/(\d+)$/u,
    (engine, [nodeId, parameterId], body) =>
      engine.setProcessorParameter(
        nodeId as string,
        Number(parameterId),
        body.value as number,
        body.expectedRevision as number,
      ),
  ],
];

/** Native commands the desktop proxies to the engine as `{ status, body }`. */
const proxiedCommands: Record<
  string,
  (engine: Required<ControlPort>, args: Body) => Promise<unknown>
> = {
  control_get_graph: (engine) => engine.getGraph(),
  control_apply_graph: (engine, args) => engine.applyGraph(args.document as never),
  control_get_graph_status: (engine) => engine.getGraphStatus(),
  control_get_status: (engine) => engine.getStatus(),
  control_get_meters: (engine) => engine.getMeters(),
  control_get_graph_catalog: (engine) => engine.getGraphCatalog(),
  control_mutate_graph_catalog: (engine, args) => engine.mutateGraphCatalog(args.request as never),
  control_get_graph_layout: (engine, args) => engine.getGraphLayout(args.graphId as string),
  control_set_graph_layout: (engine, args) =>
    engine.setGraphLayout(args.graphId as string, args.request as never),
  control_get_node_controls: (engine, args) => engine.getNodeControls(args.graphId as string),
  control_update_node_control: (engine, args) =>
    engine.updateNodeControl(args.graphId as string, args.nodeId as string, args.request as never),
  control_get_recorder_status: (engine, args) =>
    engine.recorderStatus(args.graphId as string, args.nodeId as string),
  control_set_recorder: (engine, args) => {
    const request = args.request as Body;
    return engine.controlRecorder(
      args.graphId as string,
      args.nodeId as string,
      request.request as never,
      request.expectedRevision as number,
    );
  },
  control_get_processors: (engine) => engine.getProcessors(),
  control_rescan_processors: (engine, args) => engine.rescanProcessors(args.paths as string[]),
  control_retry_processor: (engine, args) => engine.retryProcessor(args.request as never),
  control_get_processor_editor: (engine, args) => engine.getProcessorEditor(args.nodeId as string),
  control_set_processor_editor: (engine, args) =>
    engine.setProcessorEditor(
      args.nodeId as string,
      args.open as boolean,
      args.expectedRevision as number,
    ),
  control_get_processor_editor_frame: (engine, args) =>
    engine.getProcessorEditorFrame(args.nodeId as string),
  control_send_processor_editor_input: (engine, args) =>
    engine.sendProcessorEditorInput(args.nodeId as string, args.input as never),
  control_set_processor_parameter: (engine, args) =>
    engine.setProcessorParameter(
      args.nodeId as string,
      args.parameterId as number,
      args.value as number,
      args.expectedRevision as number,
    ),
};

/** Desktop commands answered directly, without the engine's status envelope. */
const directCommands: Record<
  string,
  (engine: Required<ControlPort>, args: Body) => Promise<unknown>
> = {
  recording_settings: (engine, args) =>
    engine.recordingSettings(args.graphId as string, args.nodeId as string),
  save_recording_settings: (engine, args) =>
    engine.setRecordingSettings(
      args.graphId as string,
      args.nodeId as string,
      args.settings as never,
      args.expectedRevision as number,
    ),
  list_capturable_applications: (engine) => engine.listCapturableApplications(),
  list_audio_devices: (engine) => engine.listAudioDevices(),
  list_device_app_activity: (engine) => engine.getVirtualDeviceAppActivity(),
};

/** A port error keeps its status and body on the wire; anything else is an engine fault. */
function failure(error: unknown) {
  return error instanceof ControlPortError
    ? { status: error.status, body: error.body }
    : { status: 500, body: { code: 'control_request_failed', parameters: null } };
}

async function settle(result: Promise<unknown>) {
  try {
    return { status: 200, body: (await result) ?? null };
  } catch (error) {
    return failure(error);
  }
}

/** `fetch` for HttpControlPort, answered by `engine` as the engine's HTTP API would. */
export function fakeEngineFetch(engine: ControlPort): typeof fetch {
  const port = engine as Required<ControlPort>;
  return async (input, init) => {
    const url = new URL(String(input), 'http://engine');
    const path = url.pathname.replace(/^\/engine/u, '');
    const method = init?.method ?? 'GET';
    const body = typeof init?.body === 'string' ? (JSON.parse(init.body) as Body) : {};
    for (const [candidate, pattern, call] of routes) {
      const match = candidate === method ? pattern.exec(path) : null;
      if (!match) continue;
      const params = match.slice(1).map(decodeURIComponent);
      const { status, body: payload } = await settle(call(port, params, body));
      return Response.json(payload, { status });
    }
    return Response.json({ code: 'node_not_found', parameters: null }, { status: 404 });
  };
}

/** Tauri `invoke` for TauriControlPort, answered by `engine` as the desktop would. */
export function fakeEngineInvoke(engine: ControlPort) {
  const port = engine as Required<ControlPort>;
  return async <T>(command: string, args: Body = {}): Promise<T> => {
    const proxied = proxiedCommands[command];
    if (proxied) return (await settle(proxied(port, args))) as T;
    const direct = directCommands[command];
    if (!direct) throw new Error(`unknown command ${command}`);
    return (await direct(port, args)) as T;
  };
}
