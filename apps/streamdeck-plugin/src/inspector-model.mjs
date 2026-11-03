import { actionIds, actionSettings, supportsAction } from './action-settings.mjs';
import { recordingSettingsSummary } from './recording-settings.mjs';

/** Selection and visibility state for one property inspector, independent of DOM and SDK. */
export class InspectorModel {
  snapshot;

  constructor(actionId, settings, dial, language) {
    this.actionId = actionId;
    this.settings = settings;
    this.dial = dial;
    this.language = language;
  }

  select(field, value) {
    if (field === 'graph') this.settings = { ...this.settings, graphId: value, nodeId: '' };
    else {
      const key = { node: 'nodeId', operation: 'operation', direction: 'direction', step: 'stepDb' }[field];
      if (!key) throw new Error('Unknown inspector field');
      this.settings = { ...this.settings, [key]: field === 'step' ? Number(value) : value.trim() };
    }
    return this.settings;
  }

  view() {
    const graphs = this.snapshot?.graphs ?? [];
    const nodes = (graphs.find((graph) => graph.id === this.settings.graphId)?.nodes ?? [])
      .filter((node) => supportsAction(this.actionId, node));
    const volume = this.actionId === actionIds.volume;
    return {
      graphs,
      nodes,
      selected: actionSettings(this.actionId, this.settings, this.dial),
      volume,
      recording: this.actionId === actionIds.recording,
      summary: recordingSettingsSummary(nodes.find((node) => node.nodeId === this.settings.nodeId), this.language),
      hint: nodes.length === 0 && this.settings.graphId
        ? 'No supported nodes in this graph.'
        : volume && this.dial ? 'Rotate: volume · Press: mute.' : '',
    };
  }
}
