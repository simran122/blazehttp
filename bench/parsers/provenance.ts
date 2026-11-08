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

/** The revision an image is stamped with (`org.opencontainers.image.revision`): the commit, marked when dirty. */
export function revision(tree: TreeState): string {
  return tree.dirtySha256 === null ? tree.commit : `${tree.commit}+dirty.${tree.dirtySha256.slice(0, 12)}`;
}
