import { join } from 'node:path';
import { listVectorFiles, readIndex, REPO_ROOT, VECTORS_DIR } from './schema.ts';

/** Problems with `<root>/index.json`: files on disk it does not list, listed files that are gone, files with no suite. */
export function checkIndex(root: string): string[] {
  const index = readIndex(root);
  const onDisk = listVectorFiles(root);
  const present = new Set(onDisk);
  return [
    ...onDisk.filter((f) => !Object.hasOwn(index, f)).map((f) => `unreferenced: ${f}`),
    ...Object.keys(index)
      .filter((f) => !present.has(f))
      .map((f) => `missing: ${f}`),
    ...Object.entries(index)
      .filter(([f, suites]) => present.has(f) && suites.length === 0)
      .map(([f]) => `no suite: ${f}`),
  ];
}

/* v8 ignore start -- CLI wrapper, exercised by `pnpm vectors:index` in CI */
if (import.meta.main) {
  const problems = checkIndex(join(REPO_ROOT, VECTORS_DIR));
  for (const p of problems) console.error(`${VECTORS_DIR}/index.json: ${p}`);
  process.exitCode = problems.length === 0 ? 0 : 1;
}
/* v8 ignore stop */
