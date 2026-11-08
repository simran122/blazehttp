// Cross-language response-head parser bench runner (bench/parsers/README.md).
// Usage: node bench/parsers/run.ts [--rounds 7] [--run-ms 300] [--feed-rounds 5] [--cpuset 2] [--cc gcc-14|gcc-12]
//                                  [--parsers id,id] [--out <file.jsonl>] [--no-build] [--gate-only]
import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { loadavg, tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';

import { feedFailure, gateFailures } from './gate.ts';
import type { GateParser } from './gate.ts';
import { dirtyDigest, readUntrackedAll, revision, sourceChange, staleImages, untrackedPaths } from './provenance.ts';
import type { TreeState } from './provenance.ts';
import { parseCheck, parseFeed, parseTime } from './protocol.ts';
import type { CheckReport, FeedMode } from './protocol.ts';
import { feedTable, sampleKey, timedResults, timedTable } from './report.ts';
import type { FeedResult } from './report.ts';
import { spread } from './stats.ts';
import { workloads } from './workloads.ts';
import type { Workload } from './workloads.ts';

const ROOT = resolve(import.meta.dirname, '../..');
const HERE = import.meta.dirname;
/** A harness invocation runs ≤ 0.5 s of timing (or a few seconds of quadratic feeding); anything longer is hung. */
const INVOCATION_TIMEOUT_MS = 120_000;
/** Every container this runner starts: bounded memory (swap included, so the bench host never runs out), no network. */
const CONTAINER_LIMITS = ['--memory=2g', '--memory-swap=2g', '--network=none'] as const;

/** A harness container's flags: the limits plus one CPU of quota pinned to vCPU `cpuset`; recorded in `meta.container`. */
const harnessFlags = (cpuset: string): string[] => ['--cpus=1', `--cpuset-cpus=${cpuset}`, ...CONTAINER_LIMITS];

/** One image build, downloads included; a slower build is hung (Docker Hub pulls hung for hours on the bench host). */
const BUILD_TIMEOUT_MS = 20 * 60_000;

interface Harness {
  /** Directory under bench/parsers holding the Dockerfile. */
  readonly dir: string;
  /** Build context relative to the repository root. */
  readonly context: string;
  /** Whether the image is built with the C/C++ compiler choice (`--cc`). */
  readonly compiled: boolean;
}

interface Parser extends GateParser {
  readonly language: string;
  readonly harness: Harness;
  /** What one parse does, for the "work done" column. */
  readonly work: string;
}

const harness = (dir: string, compiled = false, context = `bench/parsers/${dir}`): Harness => ({
  dir,
  context,
  compiled,
});
const RUST = harness('rust', false, '.');
const C = harness('c', true);
const CPP = harness('cpp', true);
const ZIG = harness('zig');
const GO = harness('go');

const ORDERED = { orderPreserved: true, consumed: [] } as const;
const OURS = 'blazehttp';
const OURS_WITH_FIELDS = 'blazehttp+fields';

/** Ours first (the gate's reference); every ratio is taken against one of our two rows. */
const PARSERS: readonly Parser[] = [
  {
    id: OURS,
    language: 'Rust',
    harness: RUST,
    ...ORDERED,
    work: 'validates every byte (RFC 9112 grammar); framing facts; field lines as one offset range; no callbacks',
  },
  {
    id: OURS_WITH_FIELDS,
    language: 'Rust',
    harness: RUST,
    ...ORDERED,
    work: 'blazehttp + split of the field block into name/value slices in a caller array (100), like httparse',
  },
  {
    id: 'httparse',
    language: 'Rust',
    harness: RUST,
    ...ORDERED,
    work: 'validates tokens/values; name/value slices into a caller array (100); + same framing-fact pass',
  },
  {
    id: 'picohttpparser',
    language: 'C',
    harness: C,
    ...ORDERED,
    work: 'validates tokens/values; name/value pointers into a caller array (100); + same framing-fact pass',
  },
  {
    id: 'llhttp',
    language: 'C',
    harness: C,
    ...ORDERED,
    work: 'strict state machine; framing facts built in; field callbacks record name/value spans (OWS trimmed)',
  },
  {
    id: 'beast-response-parser',
    language: 'C++',
    harness: CPP,
    ...ORDERED,
    work: 'http::response_parser<empty_body>: builds http::fields (fields + 1 allocations); facts built in',
  },
  {
    id: 'beast-basic-parser',
    language: 'C++',
    harness: CPP,
    ...ORDERED,
    work: 'basic_parser<false> subclass: one virtual call per field recording its spans in a reused list; facts built in',
  },
  {
    id: 'zig-std-http',
    language: 'Zig',
    harness: ZIG,
    ...ORDERED,
    work: 'HeadParser (finds CRLF CRLF, vectorised) + Head.parse (line split, known fields only; no token/value validation; hands no fields out)',
  },
  {
    id: 'go-net-http',
    language: 'Go',
    harness: GO,
    orderPreserved: false,
    consumed: ['connection', 'transfer-encoding'],
    work: 'http.ReadResponse: *Response + canonicalised Header map (allocates); framing facts built in',
  },
];

/** C/C++ compiler choices (`--cc`): Dockerfile defaults, or the Debian 12 / GCC 12.2 variant measured first. */
const COMPILERS: Readonly<Record<string, Readonly<Record<string, string>>>> = {
  'gcc-14': {},
  'gcc-12': {
    BASE: 'debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc',
    GCC_MAJOR: '12',
    GCC_DEB: '12.2.0-14+deb12u1',
  },
};

interface Options {
  readonly rounds: number;
  readonly runMs: number;
  readonly feedRounds: number;
  readonly cpuset: string;
  readonly cc: string;
  readonly parsers: readonly Parser[];
  readonly out: string | undefined;
  readonly build: boolean;
  /** Build and run the differential gate only: no timing, no results file. */
  readonly gateOnly: boolean;
}

/** Runs a command to completion and returns its stdout bytes; a failure, signal or timeout throws. */
function runBytes(command: string, args: readonly string[]): Buffer {
  const result = spawnSync(command, args, { cwd: ROOT, maxBuffer: 256 * 1024 * 1024, timeout: INVOCATION_TIMEOUT_MS });
  if (result.error !== undefined) throw new Error(`${command} ${args.join(' ')}: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed (${String(result.status)}):\n${result.stderr.toString()}`);
  }
  return result.stdout;
}

const run = (command: string, args: readonly string[]): string => runBytes(command, args).toString('utf8');

const imageOf = (h: Harness, cc: string): string => `blazehttp-parsers-${h.dir}${h.compiled ? `-${cc}` : ''}`;

function build(h: Harness, cc: string, source: TreeState): void {
  process.stderr.write(`building ${imageOf(h, cc)}\n`);
  const args = h.compiled ? Object.entries(COMPILERS[cc] ?? {}).flatMap(([k, v]) => ['--build-arg', `${k}=${v}`]) : [];
  const label = ['--label', `org.opencontainers.image.revision=${revision(source)}`];
  const dockerfile = join(HERE, h.dir, 'Dockerfile');
  spawnBuild(['build', '-q', ...args, ...label, '-t', imageOf(h, cc), '-f', dockerfile, join(ROOT, h.context)]);
}

/** Image builds download toolchains (tens of MB each), so they get a longer timeout than a harness invocation. */
function spawnBuild(args: readonly string[]): void {
  const result = spawnSync('docker', args, { cwd: ROOT, encoding: 'utf8', timeout: BUILD_TIMEOUT_MS });
  if (result.error !== undefined) throw new Error(`docker ${args.join(' ')}: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`docker ${args.join(' ')} failed:\n${result.stderr}`);
}

/**
 * One harness invocation in its own named container: one CPU of quota pinned to one vCPU, 2 GiB of memory, no
 * network. A container that outlives the timeout is removed (it is ours) and reported.
 */
function harnessRun(parser: Parser, options: Options, workDir: string, args: readonly string[]): string {
  const name = `blazehttp-parsers-${randomUUID()}`;
  const docker = [
    'run',
    '--rm',
    `--name=${name}`,
    ...harnessFlags(options.cpuset),
    '-v',
    `${workDir}:/work:ro`,
    imageOf(parser.harness, options.cc),
    parser.id,
    ...args,
  ];
  try {
    return run('docker', docker);
  } catch (error) {
    const removed = spawnSync('docker', ['rm', '-f', name], { encoding: 'utf8' });
    const note = removed.status === 0 && removed.stdout.trim() === name ? ' (timed out; container removed)' : '';
    throw new Error(`${parser.id} ${args.join(' ')}${note}`, { cause: error });
  }
}

const fileOf = (w: Workload): string => `/work/${w.name}.http`;
const bytesOf = (w: Workload): number => w.heads.reduce((n, head) => n + head.length, 0);

function gate(options: Options, workDir: string, all: readonly Workload[]): Map<string, Map<string, CheckReport>> {
  const reports = new Map<string, Map<string, CheckReport>>(
    options.parsers.map((parser) => [
      parser.id,
      new Map(all.map((w) => [w.name, parseCheck(harnessRun(parser, options, workDir, ['check', fileOf(w)]))])),
    ]),
  );
  const failures = gateFailures(options.parsers, all, reports, OURS);
  if (failures.length > 0) throw new Error(`differential gate failed:\n${failures.join('\n')}`);
  process.stderr.write('differential gate passed\n');
  return reports;
}

/** Round-robin timing: each round runs every parser once per workload, starting at a different parser each round. */
function timeAll(options: Options, workDir: string, timed: readonly Workload[]): Map<string, number[]> {
  const samples = new Map<string, number[]>();
  const { parsers } = options;
  for (let round = 0; round < options.rounds; round++) {
    for (const w of timed) {
      for (let k = 0; k < parsers.length; k++) {
        const parser = parsers[(round + k) % parsers.length] as Parser;
        const stdout = harnessRun(parser, options, workDir, ['time', fileOf(w), String(options.runMs)]);
        const key = sampleKey(parser.id, w.name);
        samples.set(key, [...(samples.get(key) ?? []), parseTime(stdout)]);
      }
    }
    process.stderr.write(`round ${String(round + 1)}/${String(options.rounds)} done\n`);
  }
  return samples;
}

function feedAll(options: Options, workDir: string, feeds: readonly Workload[]): FeedResult[] {
  return feeds.flatMap((w) =>
    options.parsers.map((parser): FeedResult => {
      const ms: number[] = [];
      let mode: FeedMode = 'incremental';
      for (let round = 0; round < options.feedRounds; round++) {
        const report = parseFeed(harnessRun(parser, options, workDir, ['feed', fileOf(w)]));
        const failure = feedFailure(parser.id, w, report);
        if (failure !== undefined) throw new Error(failure);
        mode = report.mode;
        ms.push(report.ns / 1e6);
      }
      return {
        kind: 'feed',
        parser: parser.id,
        language: parser.language,
        workload: w.name,
        bytes: bytesOf(w),
        rounds: options.feedRounds,
        mode,
        ms: spread(ms),
      };
    }),
  );
}

function versions(options: Options): Record<string, string> {
  const harnesses = [...new Set(options.parsers.map((p) => p.harness))];
  return Object.fromEntries(
    harnesses.map((h) => [
      h.dir,
      run('docker', [
        'run',
        '--rm',
        ...CONTAINER_LIMITS,
        '--entrypoint',
        'cat',
        imageOf(h, options.cc),
        '/versions.txt',
      ]).trim(),
    ]),
  );
}

/**
 * The working tree's state (commit, dirty files, digest of uncommitted work); `main` takes it before and after the
 * image builds.
 */
function tree(): TreeState {
  const untracked = readUntrackedAll(
    ROOT,
    untrackedPaths(runBytes('git', ['ls-files', '-z', '--others', '--exclude-standard'])),
  );
  return {
    commit: run('git', ['rev-parse', 'HEAD']).trim(),
    dirtyFiles: run('git', ['status', '--porcelain'])
      .split('\n')
      .filter((line) => line !== ''),
    // Raw bytes of the real changes: `--binary` so two binary edits differ, and no colour, external diff or textconv.
    dirtySha256: dirtyDigest(
      runBytes('git', ['diff', '--binary', '--no-color', '--no-ext-diff', '--no-textconv', 'HEAD']),
      untracked,
    ),
  };
}

/** Each image's id and labels (its `org.opencontainers.image.revision` says which tree built it). */
function images(opts: Options): Record<string, { id: string; labels: Record<string, string> }> {
  const harnesses = [...new Set(opts.parsers.map((p) => p.harness))];
  return Object.fromEntries(
    harnesses.map((h) => {
      const [image] = JSON.parse(run('docker', ['image', 'inspect', imageOf(h, opts.cc)])) as {
        Id: string;
        Config: { Labels: Record<string, string> | null };
      }[];
      if (image === undefined) throw new Error(`no image ${imageOf(h, opts.cc)}`);
      return [imageOf(h, opts.cc), { id: image.Id, labels: image.Config.Labels ?? {} }];
    }),
  );
}

function positive(text: string, what: string, min = 1): number {
  const n = Number(text);
  if (!/^\d+$/.test(text) || n < min) throw new Error(`${what} must be an integer ≥ ${String(min)}`);
  return n;
}

function options(): Options {
  const { values } = parseArgs({
    options: {
      rounds: { type: 'string', default: '7' },
      'run-ms': { type: 'string', default: '300' },
      'feed-rounds': { type: 'string', default: '5' },
      cpuset: { type: 'string', default: '2' },
      cc: { type: 'string', default: 'gcc-14' },
      parsers: { type: 'string' },
      out: { type: 'string' },
      'no-build': { type: 'boolean', default: false },
      'gate-only': { type: 'boolean', default: false },
    },
  });
  if (COMPILERS[values.cc] === undefined) throw new Error(`--cc must be one of ${Object.keys(COMPILERS).join(', ')}`);
  const wanted = values.parsers?.split(',');
  const unknown = wanted?.filter((id) => !PARSERS.some((p) => p.id === id)) ?? [];
  if (unknown.length > 0) throw new Error(`unknown parsers: ${unknown.join(', ')}`);
  return {
    rounds: positive(values.rounds, '--rounds', 5),
    runMs: positive(values['run-ms'], '--run-ms'),
    feedRounds: positive(values['feed-rounds'], '--feed-rounds'),
    cpuset: String(positive(values.cpuset, '--cpuset', 0)),
    cc: values.cc,
    // Our two rows are always included: the gate and every ratio refer to them.
    parsers: PARSERS.filter((p) => p.id === OURS || p.id === OURS_WITH_FIELDS || (wanted?.includes(p.id) ?? true)),
    out: values.out,
    build: !values['no-build'],
    gateOnly: values['gate-only'],
  };
}

function main(): void {
  const opts = options();
  const date = new Date().toISOString().slice(0, 10);
  const docker = JSON.parse(run('docker', ['info', '--format', '{{json .}}'])) as Record<string, unknown>;
  const arch = String(docker.Architecture);
  const out =
    opts.out ?? join(ROOT, 'bench/results', `${date}-${arch === 'aarch64' ? 'arm64' : arch}-dev-parsers.jsonl`);
  if (!opts.gateOnly && existsSync(out)) throw new Error(`${out} exists; pass --out to write this run elsewhere`);

  // Benchmark host rule: never share the CPUs with containers this run did not start (and never stop them).
  const running = run('docker', ['ps', '--format', '{{.Names}} {{.Image}}']).trim();
  if (running !== '') throw new Error(`containers this run did not start are running; stop them first:\n${running}`);
  const source = tree();
  if (opts.build) for (const h of new Set(opts.parsers.map((p) => p.harness))) build(h, opts.cc, source);
  // Taken again after the builds (right away with --no-build): a tree that moved meanwhile invalidates the images.
  const changed = sourceChange(source, tree());
  if (changed !== undefined) throw new Error(changed);
  const builtImages = images(opts);
  const stale = staleImages(builtImages, revision(source));
  if (stale.length > 0) {
    throw new Error(`images do not match the source tree; rebuild (drop --no-build):\n${stale.join('\n')}`);
  }

  const workDir = mkdtempSync(join(tmpdir(), 'blazehttp-parsers-'));
  try {
    const all = workloads(join(ROOT, 'tests/fixtures/h1-heads'));
    for (const w of all) writeFileSync(join(workDir, `${w.name}.http`), Buffer.concat(w.heads));
    const loadBefore = loadavg();
    const checks = gate(opts, workDir, all);
    if (opts.gateOnly) return;
    const timedWorkloads = all.filter((w) => w.kind === 'timed');
    const samples = timeAll(opts, workDir, timedWorkloads);
    const feeds = feedAll(
      opts,
      workDir,
      all.filter((w) => w.kind === 'feed'),
    );
    const loadAfter = loadavg();
    const timed = timedResults(
      opts.parsers,
      timedWorkloads,
      samples,
      (parser, workload) => checks.get(parser)?.get(workload)?.allocsPerHead ?? Number.NaN,
      { ours: OURS, oursWithFields: OURS_WITH_FIELDS },
    );
    const meta = {
      kind: 'meta',
      date,
      label: 'dev (not a gate run)',
      ...source,
      images: builtImages,
      docker: {
        serverVersion: docker.ServerVersion,
        architecture: arch,
        cpus: docker.NCPU,
        kernel: docker.KernelVersion,
        memTotal: docker.MemTotal,
      },
      container: harnessFlags(opts.cpuset).join(' '),
      cc: opts.cc,
      hostLoadAverage: { before: loadBefore, after: loadAfter },
      method: { rounds: opts.rounds, runMs: opts.runMs, warmUpMs: 100, feedRounds: opts.feedRounds },
      versions: versions(opts),
    };
    mkdirSync(dirname(out), { recursive: true });
    // `wx`: never overwrite an earlier run's results.
    writeFileSync(out, [meta, ...timed, ...feeds].map((r) => JSON.stringify(r)).join('\n') + '\n', { flag: 'wx' });
    process.stdout.write(`${timedTable(timed)}\n\n${feedTable(feeds)}\n\nwrote ${out}\n`);
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

main();
