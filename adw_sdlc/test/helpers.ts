/**
 * Shared test helpers.
 *
 * `withScopedEnv` runs `fn` with `vars` applied to `process.env`, then restores
 * each key's prior value (or unsets it if it was absent) — even if `fn` throws.
 * The provider tests use it to assert the scoped-credential boundary (e.g. that
 * an ambient `GH_TOKEN` is withheld from a provider's subprocess env) without
 * leaking env mutations across tests. Synchronous by design: the callback must
 * not be async, or the restore would run before it settles.
 */
export function withScopedEnv<T>(vars: Record<string, string>, fn: () => T): T {
  const prior = new Map<string, string | undefined>();
  for (const [key, value] of Object.entries(vars)) {
    prior.set(key, process.env[key]);
    process.env[key] = value;
  }
  try {
    return fn();
  } finally {
    for (const [key, value] of prior) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
  }
}
