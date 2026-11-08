import { describe, expect, it } from 'vitest';

import { dirtyDigest, revision } from './provenance.ts';

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
