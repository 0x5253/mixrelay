import { type RefObject, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AudioGraphModel, AudioGraphPosition } from '../../core/audio-graph/model';
import type { PlaceGraphNodes } from '../../core/audio-graph/nodePlacement';
import type {
  VirtualEndpointSettings,
  VirtualEndpointSpec,
} from '../../core/domain/control/status';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import { NameDialog } from '../../core/ui/NameDialog';
import { addRoutingNode, type GraphPresentation } from './model';
import {
  resolveVirtualEndpointAction,
  type VirtualNodeSide,
  virtualEndpointNodeRequest,
} from './virtualEndpointMenu';

interface Creation {
  owner: ControlPort;
  side: VirtualNodeSide;
  position: AudioGraphPosition;
  placeNodes?: PlaceGraphNodes;
}

function creationTemplate(settings: VirtualEndpointSettings | undefined) {
  // Only the canCreate dialog invokes confirmation; readback still rechecks live permission.
  return settings?.creationTemplate;
}

function ownsCreation(creation: Creation | null, owner: ControlPort): creation is Creation {
  return creation?.owner === owner;
}

function canCompleteCreation(
  mounted: boolean,
  current: { owner: ControlPort; creation: Creation | null; settings?: VirtualEndpointSettings },
  owner: ControlPort,
  pending: Creation,
) {
  return (
    mounted &&
    current.owner === owner &&
    current.creation === pending &&
    current.settings?.canCreate
  );
}

function creationIdentity(
  creation: Creation,
  pendingIdentity: RefObject<{ creation: Creation; id: string } | null>,
) {
  if (pendingIdentity.current?.creation !== creation)
    pendingIdentity.current = { creation, id: crypto.randomUUID() };
  return pendingIdentity.current.id;
}

function placeCreation(creation: Creation, next: AudioGraphModel) {
  return creation.placeNodes?.(next) ?? next;
}

/** Identity is allocated only on confirmation; queue retries reuse the same graph snapshot. */
export function useVirtualEndpointCreation({
  owner,
  model,
  settings,
  presentation,
  enabled,
  updateModel,
  createDevice,
}: {
  owner: ControlPort;
  model: AudioGraphModel | null;
  settings?: VirtualEndpointSettings;
  presentation?: GraphPresentation;
  enabled: boolean;
  updateModel: (model: AudioGraphModel) => void;
  createDevice?: (spec: VirtualEndpointSpec) => Promise<VirtualEndpointSpec | null>;
}) {
  const [creation, setCreation] = useState<Creation | null>(null);
  const pendingIdentity = useRef<{ creation: Creation; id: string } | null>(null);
  const [creating, setCreating] = useState(false);
  const [failed, setFailed] = useState(false);
  const mounted = useRef(false);
  const current = useRef({ owner, model, creation, settings });
  current.current = { owner, model, creation, settings };
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    setCreation((pending) => (pending?.owner === owner ? pending : null));
    setCreating(false);
  }, [owner]);
  const { t } = useTranslation();
  const tryAdd = (itemId: string, position: AudioGraphPosition, placeNodes?: PlaceGraphNodes) => {
    if (!itemId.startsWith('virtual')) return false;
    if (!enabled || !model || !settings) return true;
    const action = resolveVirtualEndpointAction(itemId, settings);
    if (!action) return true;
    if (action.kind === 'create') {
      setCreation({ owner, side: action.side, position, placeNodes });
      setFailed(false);
    } else {
      const request = virtualEndpointNodeRequest(action.side, action.spec);
      if (!request) return true;
      const next = addRoutingNode(model, request, position, presentation, settings);
      updateModel(placeNodes?.(next) ?? next);
    }
    return true;
  };
  const create = async (displayName: string) => {
    const template = creationTemplate(settings);
    if (!enabled || !model || !ownsCreation(creation, owner) || !template) return;
    if (creating) return;
    if (!createDevice) {
      setFailed(true);
      return;
    }
    const pending = creation;
    const spec = { ...template, id: creationIdentity(creation, pendingIdentity), displayName };
    setCreating(true);
    setFailed(false);
    const actual = await createDevice(spec).catch(() => null);
    if (!canCompleteCreation(mounted.current, current.current, owner, pending)) return;
    const request = actual ? virtualEndpointNodeRequest(creation.side, actual) : null;
    if (!request) {
      setFailed(true);
      setCreating(false);
      return;
    }
    const latest = current.current.model;
    if (latest) {
      const next = addRoutingNode(latest, request, creation.position, presentation, settings);
      updateModel(placeCreation(creation, next));
    }
    setCreating(false);
    setCreation(null);
  };
  return {
    tryAdd,
    dialog:
      creation?.owner === owner && settings?.canCreate ? (
        <NameDialog
          title={t('routing.nodes.newVirtualDevice')}
          initialName="MixRelay IO"
          nameLabel={t('routing.nodes.deviceName')}
          createLabel={t('common.create')}
          enabled={enabled && !creating}
          onCancel={() => {
            setCreation(null);
            setCreating(false);
          }}
          onCreate={(name) => void create(name)}
          error={failed ? t('driverManagement.changeFailed') : null}
        />
      ) : null,
  };
}
