import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterAll, describe, expect, it } from 'vitest';

import { dirtyDigest, readUntracked, revision, staleImages, untrackedPaths } from './provenance.ts';
import type { UntrackedFile } from './provenance.ts';

const file = (path: string, content: string): UntrackedFile => ({
  path: Buffer.from(path),
  kind: 'file',
  content: Buffer.from(content),
});

describe('dirtyDigest', () => {
  it('is null for a clean tree', () => {
    expect(dirtyDigest('', [])).toBeNull();
  });

  it('covers a tree dirty only through untracked files, and changes with their content and names', () => {
    const a = dirtyDigest('', [file('x.ts', '1')]);
    expect(a).toMatch(/^[0-9a-f]{64}$/);
    expect(dirtyDigest('', [file('x.ts', '2')])).not.toBe(a);
    expect(dirtyDigest('', [file('y.ts', '1')])).not.toBe(a);
    expect(dirtyDigest('diff', [file('x.ts', '1')])).not.toBe(a);
  });

  it('tells a symbolic link from a file with the same bytes', () => {
    expect(dirtyDigest('', [{ ...file('x', 'target'), kind: 'link' }])).not.toBe(
      dirtyDigest('', [file('x', 'target')]),
    );
  });
});

describe('revision', () => {
  it('is the commit for a clean tree and marks a dirty one', () => {
    expect(revision({ commit: 'abc', dirtyFiles: [], dirtySha256: null })).toBe('abc');
    expect(revision({ commit: 'abc', dirtyFiles: ['?? x'], dirtySha256: '0123456789abcdef' })).toBe(
      'abc+dirty.0123456789ab',
    );
  });
});

describe('untrackedPaths', () => {
  it('keeps every name byte for byte, including bytes that are not UTF-8, and skips nested repositories', () => {
    const notUtf8 = Buffer.from([0x6c, 0x61, 0x74, 0x69, 0x6e, 0xe9, 0x2e, 0x74, 0x78, 0x74]); // "latin\xe9.txt"
    const output = Buffer.concat([
      Buffer.from('naïve résumé.txt\0'),
      notUtf8,
      Buffer.from('\0dir/with space.ts\0vendored-repo/\0'),
    ]);
    expect(untrackedPaths(output)).toEqual([
      Buffer.from('naïve résumé.txt'),
      notUtf8,
      Buffer.from('dir/with space.ts'),
    ]);
    expect(untrackedPaths(Buffer.alloc(0))).toEqual([]);
  });
});

describe('readUntracked', () => {
  const root = mkdtempSync(join(tmpdir(), 'provenance-'));
  mkdirSync(join(root, 'dir'));
  writeFileSync(join(root, 'plain.txt'), 'bytes');
  symlinkSync('dir', join(root, 'link-to-dir'));
  symlinkSync('missing/target', join(root, 'dangling'));
  afterAll(() => {
    rmSync(root, { recursive: true, force: true });
  });

  it('hashes a file by its bytes', () => {
    expect(readUntracked(root, Buffer.from('plain.txt'))).toEqual(file('plain.txt', 'bytes'));
  });

  it('hashes a symbolic link by its target text without following it (directory and dangling targets)', () => {
    expect(readUntracked(root, Buffer.from('link-to-dir'))).toEqual({ ...file('link-to-dir', 'dir'), kind: 'link' });
    expect(readUntracked(root, Buffer.from('dangling'))).toEqual({
      ...file('dangling', 'missing/target'),
      kind: 'link',
    });
  });
});

describe('staleImages', () => {
  const label = (rev: string): Record<string, string> => ({ 'org.opencontainers.image.revision': rev });

  it('accepts images built from the current source', () => {
    expect(staleImages({ a: { labels: label('abc') }, b: { labels: label('abc') } }, 'abc')).toEqual([]);
  });

  it('reports images built from other code or with no revision label', () => {
    expect(staleImages({ a: { labels: label('old') }, b: { labels: {} }, c: { labels: label('abc') } }, 'abc')).toEqual(
      ['a: built from old, source is abc', 'b: built from an unknown revision, source is abc'],
    );
  });

  it('treats a dirty source as different from its commit', () => {
    expect(staleImages({ a: { labels: label('abc') } }, 'abc+dirty.0123456789ab')).toHaveLength(1);
  });
});

describe('untracked entries as git lists them', () => {
  const repo = mkdtempSync(join(tmpdir(), 'provenance-git-'));
  const git = (...args: string[]): Buffer => {
    const result = spawnSync('git', args, { cwd: repo });
    if (result.status !== 0) throw new Error(result.stderr.toString());
    return result.stdout;
  };
  git('init', '-q');
  mkdirSync(join(repo, 'dir'));
  writeFileSync(join(repo, 'dir', 'inner.txt'), 'inner');
  symlinkSync('dir', join(repo, 'link-to-dir'));
  symlinkSync('missing/target', join(repo, 'dangling'));
  mkdirSync(join(repo, 'nested'));
  spawnSync('git', ['init', '-q'], { cwd: join(repo, 'nested') });
  afterAll(() => {
    rmSync(repo, { recursive: true, force: true });
  });

  it('reads symlinks as links, skips the nested repository and never follows a link', () => {
    const entries = untrackedPaths(git('ls-files', '-z', '--others', '--exclude-standard')).map((path) =>
      readUntracked(repo, path),
    );
    expect(entries.map((e) => [e.path.toString(), e.kind, e.content.toString()]).sort()).toEqual([
      ['dangling', 'link', 'missing/target'],
      ['dir/inner.txt', 'file', 'inner'],
      ['link-to-dir', 'link', 'dir'],
    ]);
  });
});
