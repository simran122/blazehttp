import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { checkFile, type Violation, workspaceLintSettings } from './rules.ts';

/** Contains rule-violating strings as test fixtures. */
const EXCLUDED = new Set(['tools/policy-check/rules.test.ts']);
/** Frozen benchmark prototypes kept as evidence, not product code. */
// removal: P0 Task 14 (#2)
const EXCLUDED_DIRS = ['spike/'];
const MANIFEST = 'packages/blazehttp/package.json';

const packageVersion = existsSync(MANIFEST)
  ? (JSON.parse(readFileSync(MANIFEST, 'utf8')) as { version: string }).version
  : '0.0.0';

const ROOT_MANIFEST = 'Cargo.toml';
const workspace = workspaceLintSettings(ROOT_MANIFEST, readFileSync(ROOT_MANIFEST, 'utf8'));

const tracked = execFileSync('git', ['ls-files', '-z'], { encoding: 'utf8' })
  .split('\0')
  .filter(
    (file) =>
      file !== '' && !EXCLUDED.has(file) && !EXCLUDED_DIRS.some((dir) => file.startsWith(dir)) && existsSync(file),
  );

// A root manifest that does not parse is reported like any other violation, and nothing else is checked against it.
const violations: Violation[] =
  'rule' in workspace
    ? [workspace]
    : tracked.flatMap((file) =>
        checkFile(file, readFileSync(file, 'utf8'), { packageVersion, workspaceLints: workspace.settings }),
      );

for (const v of violations) console.error(`${v.file}:${String(v.line)} [${v.rule}] ${v.message}`);
process.exitCode = violations.length === 0 ? 0 : 1;
