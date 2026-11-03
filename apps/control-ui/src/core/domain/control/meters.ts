export interface NodeMeterSnapshot {
  nodeId: string;
  peaks: number[];
  portId: string;
  dynamics?: {
    inputPeaks: number[];
    gainReductionDb: number;
    thresholdDb: number;
  };
}

export interface MeterSnapshot {
  nodes: NodeMeterSnapshot[];
  protocolVersion: 1;
  revision: number;
}

/** One pushed meter window; the incarnation identifies the engine run that measured it. */
export interface MeterTelemetry {
  engineIncarnation: string;
  sequence: number;
  meters: MeterSnapshot;
}
