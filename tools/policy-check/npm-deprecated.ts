import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { promisify } from 'node:util';

const run = promisify(execFile);
const MAX_AGE_MONTHS = 24;
const MAX_BUFFER = 256 * 1024 * 1024;

type PnpmNode = { version: string; dependencies?: Record<string, PnpmNode> };
type PnpmProject = { dependencies?: Record<string, PnpmNode> };

/** Maps `name@version` to its name and version, over the full production tree. */
const collect = (deps: Record<string, PnpmNode> | undefined, into: Map<string, [string, string]>): void => {
  for (const [name, node] of Object.entries(deps ?? {})) {
    if (!node.version.includes(':')) into.set(`${name}@${node.version}`, [name, node.version]);
    collect(node.dependencies, into);
  }
};

const reviewed = JSON.parse(readFileSync(new URL('reviewed.json', import.meta.url), 'utf8')) as Record<string, string>;

const npmView = async (spec: string, field: string): Promise<string> =>
  (await run('npm', ['view', spec, field, '--json'], { maxBuffer: MAX_BUFFER })).stdout.trim();

const problemsFor = async (name: string, version: string): Promise<string[]> => {
  const problems: string[] = [];
  const deprecated = await npmView(`${name}@${version}`, 'deprecated');
  if (deprecated !== '') problems.push(`${name}@${version} is deprecated: ${deprecated}`);
  const times = JSON.parse(await npmView(name, 'time')) as Record<string, string>;
  const latest = Math.max(
    ...Object.entries(times)
      .filter(([key]) => key !== 'created' && key !== 'modified')
      .map(([, time]) => Date.parse(time)),
  );
  const cutoff = new Date();
  cutoff.setMonth(cutoff.getMonth() - MAX_AGE_MONTHS);
  if (latest < cutoff.getTime() && !(name in reviewed))
    problems.push(`${name} has no release in ${String(MAX_AGE_MONTHS)} months and is not in reviewed.json`);
  return problems;
};

const { stdout } = await run('pnpm', ['list', '-r', '--prod', '--json', '--depth', 'Infinity'], {
  maxBuffer: MAX_BUFFER,
});
const packages = new Map<string, [string, string]>();
for (const project of JSON.parse(stdout) as PnpmProject[]) collect(project.dependencies, packages);

const problems = (
  await Promise.all([...packages.values()].map(([name, version]) => problemsFor(name, version)))
).flat();
for (const problem of problems) console.error(problem);
process.exitCode = problems.length === 0 ? 0 : 1;
