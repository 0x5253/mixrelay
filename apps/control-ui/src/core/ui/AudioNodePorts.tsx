import styles from './AudioNodeCard.module.css';

const portIds = ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight'];

function NodePorts({ count, side }: { count: number; side: 'input' | 'output' }) {
  return (
    <span
      aria-hidden="true"
      className={`${styles.ports} ${side === 'input' ? styles.inputPorts : styles.outputPorts}`}
    >
      {portIds.slice(0, count).map((portId, portIndex) => (
        <span className={styles.port} data-port-index={portIndex} key={`${side}-${portId}`} />
      ))}
    </span>
  );
}

/** Decorative port marks for a standalone card; a graph draws its own port handles. */
export function AudioNodePorts({
  inputCount,
  outputCount,
  visible,
}: {
  inputCount: number;
  outputCount: number;
  visible: boolean;
}) {
  if (!visible) return null;
  return (
    <>
      <NodePorts count={inputCount} side="input" />
      <NodePorts count={outputCount} side="output" />
    </>
  );
}
