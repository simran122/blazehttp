import { describe, expect, it } from 'vitest';

import { dirtyDigest, revision, staleImages, untrackedPaths } from './provenance.ts';

const file = (path: string, content: string): { path: string; content: Buffer } => ({
  path,
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
  it('keeps names with any bytes and skips nested repositories', () => {
    expect(untrackedPaths('naïve résumé.txt\0dir/with space.ts\0vendored-repo/\0')).toEqual([
      'naïve résumé.txt',
      'dir/with space.ts',
    ]);
    expect(untrackedPaths('')).toEqual([]);
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
