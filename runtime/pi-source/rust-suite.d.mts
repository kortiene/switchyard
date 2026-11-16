export const workspaceLockFilters: readonly string[];

export function rustOutcome(
  output: string,
  label: string,
): { passed: number; filtered: number; names: string[] };

export function holdsWorkspaceLock(name: string, filters?: readonly string[]): boolean;

export function assertRustSplit(
  parallelOutput: string,
  serialOutput: string,
  filters?: readonly string[],
): {
  parallel: { passed: number; filtered: number; names: string[] };
  serial: { passed: number; filtered: number; names: string[] };
  rustPassed: Set<string>;
  rustTests: number;
};
