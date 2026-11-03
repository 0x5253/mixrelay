import {
  createContext,
  type Dispatch,
  type SetStateAction,
  useContext,
  useId,
  useState,
} from 'react';

export type NodeOptionsState = [string | null, Dispatch<SetStateAction<string | null>>];

/** A canvas owns one open disclosure; isolated cards keep their own state. */
export const NodeOptionsContext = createContext<NodeOptionsState | null>(null);
export const NodeOptionsIdContext = createContext<string | undefined>(undefined);

export function useNodeOptionsState() {
  const localId = useId();
  const nodeId = useContext(NodeOptionsIdContext);
  const id = nodeId ?? localId;
  const local = useState<string | null>(null);
  const [openedId, setOpenedId] = useContext(NodeOptionsContext) ?? local;
  return {
    opened: openedId === id,
    toggle: () => setOpenedId((current) => (current === id ? null : id)),
  };
}
