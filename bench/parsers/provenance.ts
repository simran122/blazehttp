// Which source produced a run: the commit, the uncommitted changes, and a stamp the images carry.
import { createHash } from 'node:crypto';

export interface UntrackedFile {
  readonly path: string;
  readonly content: Buffer;
}

export interface TreeState {
  readonly commit: string;
  /** `git status --porcelain` lines; empty for a clean tree. */
  readonly dirtyFiles: readonly string[];
  /** SHA-256 over the tracked diff and every untracked file (path and bytes); null for a clean tree. */
  readonly dirtySha256: string | null;
}

/**
 * The digest of uncommitted work: `git diff HEAD` plus each untracked file's path and content, so a tree that is dirty
 * only through new files still gets a digest that changes with them.
 */
export function dirtyDigest(diff: string, untracked: readonly UntrackedFile[]): string | null {
  if (diff === '' && untracked.length === 0) return null;
  const hash = createHash('sha256').update(diff);
  for (const file of untracked) hash.update('\0').update(file.path).update('\0').update(file.content);
  return hash.digest('hex');
}

/**
 * Untracked file paths from `git ls-files -z --others --exclude-standard`: NUL-separated, so names with any bytes
 * survive unquoted; an entry ending in `/` is a nested repository (not a file) and is skipped.
 */
export function untrackedPaths(output: string): string[] {
  return output.split('\0').filter((path) => path !== '' && !path.endsWith('/'));
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
