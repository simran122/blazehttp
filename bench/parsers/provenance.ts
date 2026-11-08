// Which source produced a run: the commit, the uncommitted changes, and a stamp the images carry.
import { createHash } from 'node:crypto';
import type { Hash } from 'node:crypto';
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
  /** SHA-256 over the tracked diff and every untracked entry (path, kind and content); null for a clean tree. */
  readonly dirtySha256: string | null;
}

/** Feeds `bytes` preceded by its length as an unsigned 64-bit big-endian integer, so no field can run into the next. */
function updateFramed(hash: Hash, bytes: Buffer): Hash {
  const length = Buffer.alloc(8);
  length.writeBigUInt64BE(BigInt(bytes.length));
  return hash.update(length).update(bytes);
}

const KIND_BYTE: Readonly<Record<UntrackedFile['kind'], number>> = { file: 1, link: 2 };

/**
 * The digest of uncommitted work, so a tree that is dirty only through new files still gets a digest that changes with
 * them. The input is a uniquely decodable stream: the length-framed diff bytes, then for each untracked entry its
 * length-framed path, one kind byte and the 32-byte SHA-256 of its content.
 */
export function dirtyDigest(diff: Buffer, untracked: readonly UntrackedFile[]): string | null {
  if (diff.length === 0 && untracked.length === 0) return null;
  const hash = updateFramed(createHash('sha256'), diff);
  for (const file of untracked) {
    updateFramed(hash, file.path)
      .update(Buffer.of(KIND_BYTE[file.kind]))
      .update(createHash('sha256').update(file.content).digest());
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
  // git terminates every entry; an unterminated tail is still a path, never dropped.
  const tail = output.subarray(start);
  if (tail.length > 0 && tail[tail.length - 1] !== SLASH) paths.push(tail);
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

/**
 * Error codes that mean an entry vanished or changed type after git listed it, so the tree is changing under us:
 * ENOENT (deleted), EISDIR (now a directory), ENOTDIR (a parent is now a file), EINVAL (`readlink` on an entry that
 * stopped being a link between `lstat` and `readlink`).
 */
const RACE_CODES: ReadonlySet<string> = new Set(['ENOENT', 'EISDIR', 'ENOTDIR', 'EINVAL']);

const raceCode = (error: unknown): string | undefined =>
  error instanceof Error && 'code' in error && typeof error.code === 'string' && RACE_CODES.has(error.code)
    ? error.code
    : undefined;

/**
 * Reads every untracked entry with `read` (`readUntracked`; injectable for tests). An entry that vanished or changed
 * type since git listed it is reported as a moving tree; any other error is rethrown unchanged.
 */
export function readUntrackedAll(
  root: string,
  paths: readonly Buffer[],
  read: (root: string, path: Buffer) => UntrackedFile = readUntracked,
): UntrackedFile[] {
  return paths.map((path) => {
    try {
      return read(root, path);
    } catch (error) {
      const code = raceCode(error);
      if (code === undefined) throw error;
      throw new Error(
        `the source tree changed while it was read (${JSON.stringify(path.toString())}: ${code}); run again on a still tree`,
        { cause: error },
      );
    }
  });
}

/** The revision an image is stamped with (`org.opencontainers.image.revision`): the commit, marked when dirty. */
export function revision(tree: TreeState): string {
  return tree.dirtySha256 === null ? tree.commit : `${tree.commit}+dirty.${tree.dirtySha256.slice(0, 12)}`;
}

/**
 * Why the source changed between two snapshots (at the start of a run and after its image builds, or after the image
 * check with `--no-build`), or undefined when it did not: images built while the tree changed may mix both versions.
 */
export function sourceChange(before: TreeState, after: TreeState): string | undefined {
  return revision(before) === revision(after)
    ? undefined
    : `the source tree changed while the images were prepared (${revision(before)} → ${revision(after)}); run again on a still tree`;
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
