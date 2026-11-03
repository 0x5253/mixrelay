export interface ProcessorPaths {
  read(): string[];
  write(paths: string[]): void;
}
