import type { ProcessorPaths } from '../../ports/ProcessorPaths';

const PATHS_KEY = 'mixrelay.processorPaths.v1';

export const browserProcessorPaths: ProcessorPaths = {
  read() {
    const saved: unknown = JSON.parse(window.localStorage.getItem(PATHS_KEY) ?? '[]');
    if (!Array.isArray(saved) || !saved.every((path) => typeof path === 'string')) {
      throw new Error('Invalid processor search paths');
    }
    return saved;
  },
  write(paths) {
    window.localStorage.setItem(PATHS_KEY, JSON.stringify(paths));
  },
};
