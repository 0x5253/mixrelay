/** Shared by the hardware action and property inspector; never invent unsupported controls. */
export function operations(node) {
  if (!node) return [];
  return [
    ...(node.capabilities.power ? ['power'] : []),
    ...(node.capabilities.mute ? ['mute'] : []),
    ...(Number.isFinite(node.gainDb) ? ['gain_up', 'gain_down'] : []),
    ...(node.capabilities.recorder ? ['record', 'pause', 'stop'] : []),
  ];
}
