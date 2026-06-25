import { describe, expect, it, vi } from 'vitest';

import { ENV_ALIASES, modelEnvAlias, readEnvAlias, readEnvFlag } from '../src/env-vars.js';

describe('ADW env aliases', () => {
  it('prefers canonical ADW vars over unset legacy aliases', () => {
    expect(readEnvAlias({ ADW_RUNNER: 'codex' }, ENV_ALIASES.runner)).toBe('codex');
    expect(readEnvFlag({ ADW_PARITY_FORCE_FENCED_JSON: '1' }, ENV_ALIASES.forceFenced)).toBe(true);
  });

  it('accepts deprecated MX_AGENT aliases with a warning', () => {
    const stderr = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    expect(readEnvAlias({ MX_AGENT_RUNNER: 'pi' }, ENV_ALIASES.runner)).toBe('pi');
    expect(stderr).toHaveBeenCalledWith(expect.stringContaining('MX_AGENT_RUNNER is deprecated; use ADW_RUNNER instead'));
    stderr.mockRestore();
  });

  it('fails loudly when canonical and legacy vars conflict', () => {
    expect(() =>
      readEnvAlias({ ADW_ENGINE: 'ts', MX_AGENT_ENGINE: 'py' }, ENV_ALIASES.engine),
    ).toThrow(/conflicting env vars: ADW_ENGINE and deprecated MX_AGENT_ENGINE/);
  });

  it('maps per-phase model overrides to ADW_MODEL_<PHASE> with MX compatibility', () => {
    const alias = modelEnvAlias('implement');
    expect(alias).toEqual({ canonical: 'ADW_MODEL_IMPLEMENT', legacy: 'MX_AGENT_MODEL_IMPLEMENT' });
    expect(readEnvAlias({ ADW_MODEL_IMPLEMENT: 'sonnet' }, alias)).toBe('sonnet');
  });
});
