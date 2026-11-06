import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, onTestFinished } from 'vitest';
import { checkIndex } from './check-index.ts';

/** A fresh temp directory, removed when the test finishes. */
const tempRoot = (): string => {
  const root = mkdtempSync(join(tmpdir(), 'vectors-'));
  onTestFinished(() => {
    rmSync(root, { recursive: true, force: true });
  });
  return root;
};

describe('checkIndex', () => {
  it('reports unreferenced and missing files', () => {
    const root = tempRoot();
    mkdirSync(join(root, 'h1'));
    writeFileSync(join(root, 'index.json'), JSON.stringify({ 'h1/kept.json': ['s'], 'h1/gone.json': ['s'] }));
    writeFileSync(join(root, 'h1', 'kept.json'), '{}');
    writeFileSync(join(root, 'h1', 'extra.json'), '{}');
    writeFileSync(join(root, 'h1', 'notes.txt'), '');
    expect(checkIndex(root)).toEqual(['unreferenced: h1/extra.json', 'missing: h1/gone.json']);
  });

  it('reports a file registered for no suite', () => {
    const root = tempRoot();
    writeFileSync(join(root, 'index.json'), JSON.stringify({ 'a.json': [] }));
    writeFileSync(join(root, 'a.json'), '{}');
    expect(checkIndex(root)).toEqual(['no suite: a.json']);
  });
});
