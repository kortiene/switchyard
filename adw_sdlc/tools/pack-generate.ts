#!/usr/bin/env node
/**
 * Generate/check project-pack prompts from neutral templates + profile.
 *
 * Default path is deterministic and CI-safe:
 *   npm run pack:generate
 *   npm run pack:check
 *
 * Optional `--llm` is an OFFLINE/build-time refinement pass. It never runs in
 * the ADW runtime pipeline; it writes reviewed prompts to `.adw/prompts` just
 * like deterministic generation. `--dry-run --llm` does not call the API.
 */

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { z } from 'zod';

import { resolveRepoPath } from '../src/config.js';
import { AdwError } from '../src/errors.js';
import {
  buildMetaprompt,
  checkPack,
  DEFAULT_PACK_DIR,
  DEFAULT_PROFILE_PATH,
  DEFAULT_TEMPLATES_DIR,
  generatePack,
  loadPackProfile,
  renderPack,
  type PackProfile,
  type RenderedFile,
} from '../src/pack-generator.js';
import { structuredCall } from '../src/structured-call.js';

const USAGE = `usage: pack-generate [--check] [--dry-run] [--llm] [--profile <path>] [--templates <dir>] [--out <dir>] [--model <id>] [--max-tokens <n>]

Generate .adw/prompts from neutral prompt templates plus .adw/pack.profile.json.

Flags:
  --check              verify generated output matches files on disk; write nothing
  --dry-run            show what would change; write nothing
  --llm                optional offline LLM refinement pass (never runtime)
  --profile <path>     profile JSON path (default: ${DEFAULT_PROFILE_PATH})
  --templates <dir>    template root (default: ${DEFAULT_TEMPLATES_DIR})
  --out <dir>          generated pack root (default: ${DEFAULT_PACK_DIR})
  --model <id>         LLM model for --llm (default: profile var pack_generation_model or claude-sonnet-4-6)
  --max-tokens <n>     output budget for each --llm prompt (default: 8192)
  -h, --help           show this help`;

interface Args {
  check: boolean;
  dryRun: boolean;
  llm: boolean;
  profilePath: string;
  templatesDir: string;
  outDir: string;
  model?: string;
  maxTokens: number;
}

function parseArgs(argv: readonly string[]): Args {
  const args: Args = {
    check: false,
    dryRun: false,
    llm: false,
    profilePath: DEFAULT_PROFILE_PATH,
    templatesDir: DEFAULT_TEMPLATES_DIR,
    outDir: DEFAULT_PACK_DIR,
    maxTokens: 8192,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i]!;
    const value = (flag: string): string => {
      const next = argv[i + 1];
      if (next === undefined || next.startsWith('-')) {
        throw new AdwError(`${flag} requires a value`);
      }
      i += 1;
      return next;
    };
    switch (arg) {
      case '-h':
      case '--help':
        process.stdout.write(`${USAGE}\n`);
        process.exit(0);
      case '--check':
        args.check = true;
        break;
      case '--dry-run':
        args.dryRun = true;
        break;
      case '--llm':
        args.llm = true;
        break;
      case '--profile':
        args.profilePath = value(arg);
        break;
      case '--templates':
        args.templatesDir = value(arg);
        break;
      case '--out':
        args.outDir = value(arg);
        break;
      case '--model':
        args.model = value(arg);
        break;
      case '--max-tokens': {
        const parsed = Number(value(arg));
        if (!Number.isInteger(parsed) || parsed <= 0) {
          throw new AdwError('--max-tokens expects a positive integer');
        }
        args.maxTokens = parsed;
        break;
      }
      default:
        throw new AdwError(`unknown flag: ${arg}`);
    }
  }
  if (args.check && args.llm) {
    throw new AdwError('--check is deterministic and cannot be combined with --llm; commit LLM output, then review it manually');
  }
  return args;
}

const RefinedPromptSchema = z.object({
  prompt: z.string().min(1),
  rationale: z.string().default(''),
});

async function refineWithLlm(file: RenderedFile, profile: PackProfile, model: string, maxTokens: number): Promise<string> {
  const { value } = await structuredCall(buildMetaprompt(file, profile), RefinedPromptSchema, {
    model,
    maxTokens,
  });
  return ensureTrailingNewline(value.prompt);
}

async function generateWithLlm(args: Args, profile: PackProfile): Promise<number> {
  const rendered = renderPack({ templatesDir: args.templatesDir, outDir: args.outDir, profile });
  const model = args.model ?? profile.vars['pack_generation_model'] ?? 'claude-sonnet-4-6';
  if (args.dryRun) {
    process.stdout.write(
      `would refine ${rendered.length} prompt(s) with ${model}; dry-run does not call the LLM or write files\n`,
    );
    return 0;
  }

  const outDir = resolveRepoPath(args.outDir);
  mkdirSync(outDir, { recursive: true });
  const changed: string[] = [];
  for (const file of rendered) {
    process.stderr.write(`>> refining ${file.name} with ${model}\n`);
    const prompt = await refineWithLlm(file, profile, model, args.maxTokens);
    if (readIfExists(file.outPath) !== prompt) {
      changed.push(file.name);
      writeFileSync(file.outPath, prompt, 'utf8');
    }
  }
  process.stdout.write(`llm generation complete: ${changed.length} changed, ${rendered.length - changed.length} unchanged\n`);
  if (changed.length > 0) {
    process.stdout.write(`${changed.map((name) => `  ${join(args.outDir, name)}`).join('\n')}\n`);
  }
  return 0;
}

export async function main(argv: readonly string[]): Promise<number> {
  try {
    const args = parseArgs(argv);
    const profile = loadPackProfile(resolveRepoPath(args.profilePath));

    if (args.check) {
      const result = checkPack({ templatesDir: args.templatesDir, outDir: args.outDir, profile });
      if (result.ok) {
        process.stdout.write('pack prompts are up to date\n');
        return 0;
      }
      if (result.missing.length > 0) {
        process.stderr.write(`missing generated prompt(s): ${result.missing.join(', ')}\n`);
      }
      if (result.drifted.length > 0) {
        process.stderr.write(`drifted generated prompt(s): ${result.drifted.join(', ')}\n`);
      }
      process.stderr.write('run: npm run pack:generate\n');
      return 1;
    }

    if (args.llm) {
      return await generateWithLlm(args, profile);
    }

    const result = generatePack({
      templatesDir: args.templatesDir,
      outDir: args.outDir,
      profile,
      dryRun: args.dryRun,
    });
    const prefix = args.dryRun ? 'would update' : 'updated';
    process.stdout.write(`${prefix}: ${result.written.length}; unchanged: ${result.unchanged.length}\n`);
    if (result.written.length > 0) {
      process.stdout.write(`${result.written.map((name) => `  ${join(args.outDir, name)}`).join('\n')}\n`);
    }
    return 0;
  } catch (err) {
    if (err instanceof AdwError) {
      process.stderr.write(`error: ${err.message}\n`);
      return 2;
    }
    throw err;
  }
}

function ensureTrailingNewline(text: string): string {
  return text.endsWith('\n') ? text : `${text}\n`;
}

function readIfExists(path: string): string | null {
  try {
    return readFileSync(path, 'utf8');
  } catch {
    return null;
  }
}

if (process.argv[1] !== undefined && process.argv[1] === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).then((rc) => {
    process.exitCode = rc;
  });
}
