import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    server: {
      deps: {
        // runner-codex-spawn.test.ts asserts the secret boundary on the env
        // the REAL SDK builds for its child, which requires the test's
        // child_process mock to apply INSIDE the SDK — possible only when the
        // package is inlined through vitest's module runner instead of being
        // externalized to native Node import.
        inline: ['@openai/codex-sdk'],
      },
    },
    coverage: {
      provider: 'v8',
      // `enabled` stays at its default (false): only `vitest run --coverage`
      // (the `coverage` script / verify stage) collects + enforces. A focused
      // `npx vitest run <file>` stays coverage-free and never false-fails.
      include: ['src/**/*.ts'],
      // `all: true` so EVERY file matched by `include` is reported — a brand-new
      // src module with no test shows up at 0% and drags the metric down. This
      // is the "an untested new branch would not trip the gate" gap (issue #36).
      all: true,
      // Measure real logic; don't pad the number by excluding source. Keep
      // excludes to the barrel + type decls.
      exclude: ['src/index.ts', '**/*.d.ts'],
      reporter: ['text-summary', 'text', 'html'],
      reportsDirectory: './coverage',
      thresholds: {
        // MODEST, MEASURED floor — a few points below the current baseline
        // (statements 88.9 / branches 79.1 / functions 87.9 / lines 89.0) so
        // routine churn doesn't flake the gate, while a wholly untested new
        // module still trips it.
        statements: 85,
        branches: 73,
        functions: 82,
        lines: 85,
        autoUpdate: false, // never silently lower the bar
      },
    },
  },
});
