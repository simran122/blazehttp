// Which source produced a run: the commit, the uncommitted changes, and a stamp the images carry.
import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, readlinkSync } from 'node:fs';
import { join } from 'node:path';

/** One untracked entry as git would store it: a file's bytes, or a symbolic link's target text (never followed). */
export interface UntrackedFile {
  /** The path's raw bytes, as `git ls-files -z` prints them (they need not be valid UTF-8). */
  readonly path: Buffer;
  readonly kind: 'file' | 'link';
  readonly content: Buffer;
}

export interface TreeState {
  readonly commit: string;
  /** `git status --porcelain` lines; empty for a clean tree. */
  readonly dirtyFiles: readonly string[];
  /** SHA-256 over the tracked diff and every untracked entry (path, kind and bytes); null for a clean tree. */
  readonly dirtySha256: string | null;
}

/**
 * The digest of uncommitted work: `git diff HEAD` plus each untracked entry's path, kind and content, so a tree that
 * is dirty only through new files still gets a digest that changes with them.
 */
export function dirtyDigest(diff: string, untracked: readonly UntrackedFile[]): string | null {
  if (diff === '' && untracked.length === 0) return null;
  const hash = createHash('sha256').update(diff);
  for (const file of untracked) {
    hash.update('\0').update(file.path).update('\0').update(file.kind).update('\0').update(file.content);
  }
  return hash.digest('hex');
}

const NUL = 0;
const SLASH = 0x2f;

/**
 * Untracked paths from `git ls-files -z --others --exclude-standard`, read as bytes: NUL-separated, so a name keeps
 * its exact bytes even when they are not UTF-8. An entry ending in `/` is a nested repository (not a file) and is
 * skipped.
 */
export function untrackedPaths(output: Buffer): Buffer[] {
  const paths: Buffer[] = [];
  let start = 0;
  for (let end = output.indexOf(NUL); end >= 0; end = output.indexOf(NUL, start)) {
    const path = output.subarray(start, end);
    if (path.length > 0 && path[path.length - 1] !== SLASH) paths.push(path);
    start = end + 1;
  }
  return paths;
}

/**
 * Reads one untracked entry under `root` the way git hashes it: a symbolic link's target text (never followed, so a
 * link to a directory or a dangling link works), otherwise the file's bytes.
 */
export function readUntracked(root: string, path: Buffer): UntrackedFile {
  const full = Buffer.concat([Buffer.from(join(root, '/')), path]);
  return lstatSync(full).isSymbolicLink()
    ? { path, kind: 'link', content: readlinkSync(full, { encoding: 'buffer' }) }
    : { path, kind: 'file', content: readFileSync(full) };
}

/** The revision an image is stamped with (`org.opencontainers.image.revision`): the commit, marked when dirty. */
export function revision(tree: TreeState): string {
  return tree.dirtySha256 === null ? tree.commit : `${tree.commit}+dirty.${tree.dirtySha256.slice(0, 12)}`;
}

const REVISION_LABEL = 'org.opencontainers.image.revision';

/**
 * Images built from another source than `expected` (the current tree's `revision`), as messages; empty when every
 * image matches. A `--no-build` run must not time images of other code.
 */
export function staleImages(
  images: Readonly<Record<string, { readonly labels: Readonly<Record<string, string>> }>>,
  expected: string,
): string[] {
  return Object.entries(images)
    .filter(([, image]) => image.labels[REVISION_LABEL] !== expected)
    .map(
      ([name, image]) =>
        `${name}: built from ${image.labels[REVISION_LABEL] ?? 'an unknown revision'}, source is ${expected}`,
    );
}
