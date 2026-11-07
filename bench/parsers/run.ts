// Cross-language response-head parser bench runner (bench/parsers/README.md).
// Usage: node bench/parsers/run.ts [--rounds 7] [--run-ms 300] [--feed-rounds 5] [--out <file.jsonl>] [--no-build]
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { loadavg, tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';

import { differences, parseCheck, parseFeed, parseTime } from './protocol.ts';
import type { CheckReport, FeedMode } from './protocol.ts';
import { feedTable, timedTable } from './report.ts';
import type { FeedResult, TimedResult } from './report.ts';
import { ratios, spread } from './stats.ts';
import { workloads } from './workloads.ts';
import type { Workload } from './workloads.ts';

const ROOT = resolve(import.meta.dirname, '../..');
const HERE = import.meta.dirname;

interface Harness {
  /** Directory under bench/parsers holding the Dockerfile. */
  readonly dir: string;
  /** Build context relative to the repository root. */
  readonly context: string;
}

interface Parser {
  readonly id: string;
  readonly language: string;
  readonly harness: Harness;
  /** False when the parser keeps fields in a map (names canonicalised, order lost): the gate compares multisets. */
  readonly orderPreserved: boolean;
  /** What one parse does, for the "work done" column. */
  readonly work: string;
}

const harness = (dir: string, context = `bench/parsers/${dir}`): Harness => ({ dir, context });
const RUST = harness('rust', '.');
const C = harness('c');
const CPP = harness('cpp');
const ZIG = harness('zig');
const GO = harness('go');

/** Ours first: every ratio is taken against it. */
const PARSERS: readonly Parser[] = [
  {
    id: 'blazehttp',
    language: 'Rust',
    harness: RUST,
    orderPreserved: true,
    work: 'validates every byte (RFC 9112 grammar); offsets only (field-line range); framing facts folded; no callbacks',
  },
  {
    id: 'httparse',
    language: 'Rust',
    harness: RUST,
    orderPreserved: true,
    work: 'validates tokens/values; name/value slices into a caller array (100); + same framing-fact pass',
  },
  {
    id: 'picohttpparser',
    language: 'C',
    harness: C,
    orderPreserved: true,
    work: 'validates tokens/values; name/value pointers into a caller array (100); + same framing-fact pass',
  },
  {
    id: 'llhttp',
    language: 'C',
    harness: C,
    orderPreserved: true,
    work: 'strict state machine; framing facts built in; only on_headers_complete registered (no field callbacks)',
  },
  {
    id: 'beast-response-parser',
    language: 'C++',
    harness: CPP,
    orderPreserved: true,
    work: 'http::response_parser<empty_body>: builds http::fields (one allocation per field); framing facts built in',
  },
  {
    id: 'beast-basic-parser',
    language: 'C++',
    harness: CPP,
    orderPreserved: true,
    work: 'basic_parser<false> subclass: one virtual call per field (empty), no container; framing facts built in',
  },
  {
    id: 'zig-std-http',
    language: 'Zig',
    harness: ZIG,
    orderPreserved: true,
    work: 'HeadParser (finds CRLF CRLF, vectorised) + Head.parse (line split, known fields only; no token/value validation)',
  },
  {
    id: 'go-net-http',
    language: 'Go',
    harness: GO,
    orderPreserved: false,
    work: 'http.ReadResponse: *Response + canonicalised Header map (allocates); framing facts built in',
  },
];

const imageOf = (h: Harness): string => `blazehttp-parsers-${h.dir}`;

function run(command: string, args: readonly string[], cwd = ROOT): string {
  const result = spawnSync(command, args, { cwd, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed (${String(result.status)}):\n${result.stderr}`);
  }
  return result.stdout;
}

function build(h: Harness): void {
  process.stderr.write(`building ${imageOf(h)}\n`);
  run('docker', ['build', '-q', '-t', imageOf(h), '-f', join(HERE, h.dir, 'Dockerfile'), join(ROOT, h.context)]);
}

/** One harness invocation in its own container, limited to one CPU, without network. */
function harnessRun(parser: Parser, workDir: string, args: readonly string[]): string {
  return run('docker', [
    'run',
    '--rm',
    '--cpus=1',
    '--network=none',
    '-v',
    `${workDir}:/work:ro`,
    imageOf(parser.harness),
    parser.id,
    ...args,
  ]);
}

const fileOf = (w: Workload): string => `/work/${w.name}.http`;
const bytesOf = (w: Workload): number => w.heads.reduce((n, head) => n + head.length, 0);

/** The differential gate: every parser must report the same heads as ours on every workload, before any timing. */
function gate(workDir: string, all: readonly Workload[]): Map<string, Map<string, CheckReport>> {
  const reports = new Map<string, Map<string, CheckReport>>();
  const [ours] = PARSERS;
  if (ours === undefined) throw new Error('no parsers');
  const failures: string[] = [];
  for (const parser of PARSERS) {
    const byWorkload = new Map<string, CheckReport>();
    for (const w of all) {
      const report = parseCheck(harnessRun(parser, workDir, ['check', fileOf(w)]));
      byWorkload.set(w.name, report);
      const reference = reports.get(ours.id)?.get(w.name) ?? report;
      if (reference.heads.length !== w.heads.length) failures.push(`${parser.id} ${w.name}: wrong head count`);
      const diff = differences(reference.heads, report.heads, parser.orderPreserved);
      failures.push(...diff.map((d) => `${parser.id} ${w.name}: ${d}`));
    }
    reports.set(parser.id, byWorkload);
  }
  if (failures.length > 0) throw new Error(`differential gate failed:\n${failures.join('\n')}`);
  process.stderr.write('differential gate passed\n');
  return reports;
}

/** Round-robin timing: each round runs every parser once per workload, starting at a different parser each round. */
function timeAll(workDir: string, timed: readonly Workload[], rounds: number, runMs: number): Map<string, number[]> {
  const samples = new Map<string, number[]>();
  for (let round = 0; round < rounds; round++) {
    for (const w of timed) {
      for (let k = 0; k < PARSERS.length; k++) {
        const parser = PARSERS[(round + k) % PARSERS.length];
        if (parser === undefined) continue;
        const ns = parseTime(harnessRun(parser, workDir, ['time', fileOf(w), String(runMs)]));
        const key = `${parser.id}/${w.name}`;
        samples.set(key, [...(samples.get(key) ?? []), ns]);
      }
    }
    process.stderr.write(`round ${String(round + 1)}/${String(rounds)} done\n`);
  }
  return samples;
}

function feedAll(workDir: string, feeds: readonly Workload[], rounds: number): FeedResult[] {
  return feeds.flatMap((w) =>
    PARSERS.map((parser): FeedResult => {
      const ms: number[] = [];
      let mode: FeedMode = 'incremental';
      for (let round = 0; round < rounds; round++) {
        const report = parseFeed(harnessRun(parser, workDir, ['feed', fileOf(w)]));
        if (report.headLen !== bytesOf(w))
          throw new Error(`${parser.id} ${w.name}: head length ${String(report.headLen)}`);
        mode = report.mode;
        ms.push(report.ns / 1e6);
      }
      return {
        kind: 'feed',
        parser: parser.id,
        language: parser.language,
        workload: w.name,
        bytes: bytesOf(w),
        rounds,
        mode,
        ms: spread(ms),
      };
    }),
  );
}

function versions(): Record<string, string> {
  const dirs = [...new Set(PARSERS.map((p) => p.harness))];
  return Object.fromEntries(
    dirs.map((h) => [h.dir, run('docker', ['run', '--rm', '--entrypoint', 'cat', imageOf(h), '/versions.txt']).trim()]),
  );
}

function main(): void {
  const { values } = parseArgs({
    options: {
      rounds: { type: 'string', default: '7' },
      'run-ms': { type: 'string', default: '300' },
      'feed-rounds': { type: 'string', default: '5' },
      out: { type: 'string' },
      'no-build': { type: 'boolean', default: false },
    },
  });
  const rounds = Number(values.rounds);
  const runMs = Number(values['run-ms']);
  const feedRounds = Number(values['feed-rounds']);
  if (![rounds, runMs, feedRounds].every((n) => Number.isInteger(n) && n > 0) || rounds < 5) {
    throw new Error('--rounds must be an integer ≥ 5; --run-ms and --feed-rounds positive integers');
  }
  const date = new Date().toISOString().slice(0, 10);
  const docker = JSON.parse(run('docker', ['info', '--format', '{{json .}}'])) as Record<string, unknown>;
  const arch = String(docker.Architecture);
  const out =
    values.out ?? join(ROOT, 'bench/results', `${date}-${arch === 'aarch64' ? 'arm64' : arch}-dev-parsers.jsonl`);

  process.stderr.write(
    `running containers before the run:\n${run('docker', ['ps', '--format', '{{.Names}} {{.Image}}'])}\n`,
  );
  if (!values['no-build']) for (const h of new Set(PARSERS.map((p) => p.harness))) build(h);

  const workDir = mkdtempSync(join(tmpdir(), 'blazehttp-parsers-'));
  try {
    const all = workloads(join(ROOT, 'tests/fixtures/h1-heads'));
    for (const w of all) writeFileSync(join(workDir, `${w.name}.http`), Buffer.concat(w.heads));
    const loadBefore = loadavg();
    const checks = gate(workDir, all);
    const timedWorkloads = all.filter((w) => w.kind === 'timed');
    const samples = timeAll(workDir, timedWorkloads, rounds, runMs);
    const feeds = feedAll(
      workDir,
      all.filter((w) => w.kind === 'feed'),
      feedRounds,
    );
    const loadAfter = loadavg();

    const timed = timedWorkloads.flatMap((w) =>
      PARSERS.map((parser): TimedResult => {
        const ns = samples.get(`${parser.id}/${w.name}`) ?? [];
        const ours = samples.get(`${PARSERS[0]?.id ?? ''}/${w.name}`) ?? [];
        return {
          kind: 'timed',
          parser: parser.id,
          language: parser.language,
          workload: w.name,
          heads: w.heads.length,
          bytes: bytesOf(w),
          rounds,
          nsPerHead: spread(ns),
          oursOverTheirs: spread(ratios(ours, ns)),
          allocsPerHead: checks.get(parser.id)?.get(w.name)?.allocsPerHead ?? Number.NaN,
          work: parser.work,
        };
      }),
    );
    const meta = {
      kind: 'meta',
      date,
      label: 'dev (not a gate run)',
      commit: run('git', ['rev-parse', 'HEAD']).trim(),
      dirty: run('git', ['status', '--porcelain']).trim() !== '',
      docker: {
        serverVersion: docker.ServerVersion,
        architecture: arch,
        cpus: docker.NCPU,
        kernel: docker.KernelVersion,
        memTotal: docker.MemTotal,
      },
      container: '--cpus=1 --network=none',
      hostLoadAverage: { before: loadBefore, after: loadAfter },
      method: { rounds, runMs, warmUpMs: 100, feedRounds },
      versions: versions(),
    };
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, [meta, ...timed, ...feeds].map((r) => JSON.stringify(r)).join('\n') + '\n');
    process.stdout.write(`${timedTable(timed)}\n\n${feedTable(feeds)}\n\nwrote ${out}\n`);
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

main();
