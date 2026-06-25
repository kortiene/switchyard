import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { beforeAll, describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';

const LEDGER = join(REPO_ROOT, 'adw_sdlc', 'docs', 'OBSERVED-LIVE-LEDGER.md');
const MVP_READINESS = join(REPO_ROOT, 'adw_sdlc', 'MVP-READINESS.md');
const PARITY_MD = join(REPO_ROOT, 'adw_sdlc', 'PARITY.md');

// The canonical 13 PARITY.md Section-10 guarantees (PARITY.md:24–38), keyed by a
// stable substring of each row name. The list length is asserted so a future
// PARITY edit that adds/removes a guarantee forces this ledger to keep pace.
const SECTION_10_GUARANTEES = [
  'Phase order',
  'model routing',
  'edits the worktree',
  'Structured output',
  'Secret withholding',
  'Sandboxed',
  'squash-merge',
  'Bounded loops',
  'Resume',
  'Artifacts',
  'State equivalence',
  'Cost/usage',
  'adw/ green',
] as const;

// The observed-live legend vocabulary (§ status legend).
const LEGEND_TOKENS = ['✅', '🟡', '⏳', 'N/A'] as const;

describe('docs/OBSERVED-LIVE-LEDGER.md — issue #3 acceptance criteria', () => {
  let content: string;
  beforeAll(() => {
    content = readFileSync(LEDGER, 'utf8');
  });

  it('ledger exists and is non-empty', () => {
    expect(content.length).toBeGreaterThan(500);
  });

  it('lists every PARITY Section-10 guarantee (and exactly 13)', () => {
    expect(SECTION_10_GUARANTEES).toHaveLength(13);
    for (const guarantee of SECTION_10_GUARANTEES) {
      expect(content, `missing Section-10 guarantee: ${guarantee}`).toContain(guarantee);
    }
  });

  it('every data row carries an explicit observed-live status token', () => {
    const dataRows = content
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => line.startsWith('|'))
      // drop the markdown separator row (only |, -, :, spaces)
      .filter((line) => !/^\|[\s:|-]+\|?$/.test(line))
      // drop the header row
      .filter((line) => !/Observed-live\?/i.test(line));

    // Section 10 has exactly 13 guarantees — no more, no fewer
    expect(dataRows).toHaveLength(13);
    for (const row of dataRows) {
      expect(
        LEGEND_TOKENS.some((token) => row.includes(token)),
        `row has no explicit status token: ${row}`,
      ).toBe(true);
    }
  });

  it('defines the full observed-live legend vocabulary', () => {
    for (const token of LEGEND_TOKENS) {
      expect(content).toContain(token);
    }
  });

  it('cites the seed run evidence (PR #331 / run 007fd5ba)', () => {
    expect(content).toContain('007fd5ba');
    expect(content).toContain('#331');
  });

  it('has all required document sections', () => {
    expect(content).toMatch(/## Status legend/i);
    expect(content).toMatch(/## Ledger/);
    expect(content).toMatch(/## Seed source/i);
    expect(content).toMatch(/## How to update/i);
  });

  it('has a Last-updated date marker', () => {
    expect(content).toMatch(/_Last updated:/);
  });

  it('headline tally line is present and its counts sum to 13', () => {
    // Match: "Headline ... N `✅`, N `🟡`, N `⏳`, N `N/A`"
    const re =
      /Headline[^\n]*?(\d+)\s*`✅`[^\n]*?(\d+)\s*`🟡`[^\n]*?(\d+)\s*`⏳`[^\n]*?(\d+)\s*`N\/A`/;
    const match = content.match(re);
    expect(match, 'headline tally line must contain four counts').toBeTruthy();
    if (match) {
      const total = [1, 2, 3, 4].reduce((acc, i) => acc + parseInt(match[i] ?? '0', 10), 0);
      expect(total, 'tally counts must sum to 13 (one per Section-10 guarantee)').toBe(13);
    }
  });

  it('headline tally per-token counts match the actual observed-live column', () => {
    const dataRows = content
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => line.startsWith('|'))
      .filter((line) => !/^\|[\s:|-]+\|?$/.test(line))
      .filter((line) => !/Observed-live\?/i.test(line));

    // The "Observed-live?" column is the 4th pipe-separated cell (index 4 after leading '').
    const observedCells = dataRows.map((row) => row.split('|')[4]?.trim() ?? '');

    const actual = {
      green: observedCells.filter((c) => c.startsWith('✅')).length,
      partial: observedCells.filter((c) => c.startsWith('🟡')).length,
      owed: observedCells.filter((c) => c.startsWith('⏳')).length,
      na: observedCells.filter((c) => c.startsWith('N/A')).length,
    };

    const re =
      /Headline[^\n]*?(\d+)\s*`✅`[^\n]*?(\d+)\s*`🟡`[^\n]*?(\d+)\s*`⏳`[^\n]*?(\d+)\s*`N\/A`/;
    const match = content.match(re);
    expect(match, 'headline tally must be parseable for cross-check').toBeTruthy();
    if (match) {
      expect(actual.green, '✅ observed-live rows').toBe(parseInt(match[1] ?? '0', 10));
      expect(actual.partial, '🟡 partial rows').toBe(parseInt(match[2] ?? '0', 10));
      expect(actual.owed, '⏳ not-yet-observed rows').toBe(parseInt(match[3] ?? '0', 10));
      expect(actual.na, 'N/A rows').toBe(parseInt(match[4] ?? '0', 10));
    }
  });
});

describe('MVP-READINESS.md links the observed-live ledger', () => {
  it('references docs/OBSERVED-LIVE-LEDGER.md', () => {
    const content = readFileSync(MVP_READINESS, 'utf8');
    expect(content).toContain('OBSERVED-LIVE-LEDGER.md');
  });

  it('contains a proper markdown hyperlink to the ledger (not only a text mention)', () => {
    const content = readFileSync(MVP_READINESS, 'utf8');
    // OBSERVED-LIVE-LEDGER.md must appear as a link target inside (...), not only as plain text.
    expect(content).toMatch(/\(.*?OBSERVED-LIVE-LEDGER\.md.*?\)/);
  });
});

// ---------------------------------------------------------------------------
// Cross-document sync: ledger must stay one-to-one with PARITY.md Section 10.
// The unit tests above use a hardcoded guarantee list; these tests parse
// PARITY.md directly so a new/removed guarantee causes an immediate failure
// rather than a silent ledger drift.
// ---------------------------------------------------------------------------

/** Extract data rows from PARITY.md's "Section 10 parity checklist" table. */
function extractParitySection10Rows(parityContent: string): string[] {
  const sectionStart = parityContent.indexOf('## Section 10 parity checklist');
  if (sectionStart === -1) return [];
  const sectionEnd = parityContent.indexOf('\n---\n', sectionStart);
  const section =
    sectionEnd === -1 ? parityContent.slice(sectionStart) : parityContent.slice(sectionStart, sectionEnd);
  return section
    .split('\n')
    .filter((line) => line.trim().startsWith('|'))
    .filter((line) => !/^\|[\s:|-]+\|?$/.test(line.trim()))
    // drop the header row (| Box | Status | Proven by |)
    .filter((line) => !/\|\s*Box\s*\|/i.test(line));
}

/** Extract data rows from the OBSERVED-LIVE-LEDGER.md table. */
function extractLedgerDataRows(ledgerContent: string): string[] {
  return ledgerContent
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.startsWith('|'))
    .filter((line) => !/^\|[\s:|-]+\|?$/.test(line))
    .filter((line) => !/Observed-live\?/i.test(line));
}

describe('cross-document sync: ledger mirrors PARITY.md Section 10 (e2e)', () => {
  let parityContent: string;
  let ledgerContent: string;

  beforeAll(() => {
    parityContent = readFileSync(PARITY_MD, 'utf8');
    ledgerContent = readFileSync(LEDGER, 'utf8');
  });

  it('PARITY.md contains a Section 10 parity checklist section', () => {
    expect(parityContent).toContain('## Section 10 parity checklist');
  });

  it('PARITY.md Section 10 has exactly 13 guarantee rows', () => {
    const rows = extractParitySection10Rows(parityContent);
    expect(
      rows,
      `Section 10 should have 13 rows but found ${rows.length}:\n${rows.join('\n')}`,
    ).toHaveLength(13);
  });

  it('ledger row count equals PARITY.md Section 10 guarantee count (one-to-one mapping)', () => {
    const parityRows = extractParitySection10Rows(parityContent);
    const ledgerRows = extractLedgerDataRows(ledgerContent);
    expect(
      ledgerRows,
      `ledger has ${ledgerRows.length} rows but PARITY.md Section 10 has ${parityRows.length} — they must stay in sync`,
    ).toHaveLength(parityRows.length);
  });

  it('PARITY.md Section 10 bold guarantee names each appear in the ledger', () => {
    // Extract bold guarantee names (**...**) from the first cell of each PARITY row.
    // The bold text before " — " is the canonical guarantee name used in the ledger.
    const parityRows = extractParitySection10Rows(parityContent);
    for (const row of parityRows) {
      const boldMatch = row.match(/\*\*([^*]+)\*\*/);
      if (!boldMatch) continue;
      // Take only the part before the first " — " to strip the inline description suffix.
      const fullName = boldMatch[1]?.trim() ?? '';
      const guaranteeName = fullName.split(' — ')[0]?.trim() ?? fullName;
      // Use the first few words as a stable search key (immune to minor trailing edits).
      const searchKey = guaranteeName.split(/[\s(]/)[0] ?? guaranteeName;
      expect(
        ledgerContent,
        `PARITY Section-10 guarantee "${guaranteeName}" (key: "${searchKey}") not found in ledger`,
      ).toContain(searchKey);
    }
  });

  it('ledger guarantee rows are in the same order as PARITY.md Section 10 rows (e2e order check)', () => {
    const parityRows = extractParitySection10Rows(parityContent);
    const ledgerRows = extractLedgerDataRows(ledgerContent);

    // Each PARITY guarantee key (first word of the bold name) must appear in the
    // corresponding ledger row at the same index, enforcing one-to-one order.
    parityRows.forEach((parityRow, i) => {
      const boldMatch = parityRow.match(/\*\*([^*]+)\*\*/);
      if (!boldMatch) return;
      const fullName = boldMatch[1]?.trim() ?? '';
      const guaranteeName = fullName.split(' — ')[0]?.trim() ?? fullName;
      const searchKey = (guaranteeName.split(/[\s(]/)[0] ?? guaranteeName).toLowerCase();

      expect(
        ledgerRows[i]?.toLowerCase(),
        `PARITY Section-10 row ${i + 1} key "${searchKey}" not found in ledger row ${i + 1} — rows may be out of order`,
      ).toContain(searchKey);
    });
  });
});
