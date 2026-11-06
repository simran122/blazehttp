import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { checkIndex } from './check-index.ts';

describe('checkIndex', () => {
  it('reports unreferenced and missing files', () => {
    const root = mkdtempSync(join(tmpdir(), 'vectors-'));
    mkdirSync(join(root, 'h1'));
    writeFileSync(join(root, 'index.json'), JSON.stringify({ 'h1/kept.json': ['s'], 'h1/gone.json': ['s'] }));
    writeFileSync(join(root, 'h1', 'kept.json'), '{}');
    writeFileSync(join(root, 'h1', 'extra.json'), '{}');
    writeFileSync(join(root, 'h1', 'notes.txt'), '');
    expect(checkIndex(root)).toEqual(['unreferenced: h1/extra.json', 'missing: h1/gone.json']);
  });

  it('reports a file registered for no suite', () => {
    const root = mkdtempSync(join(tmpdir(), 'vectors-'));
    writeFileSync(join(root, 'index.json'), JSON.stringify({ 'a.json': [] }));
    writeFileSync(join(root, 'a.json'), '{}');
    expect(checkIndex(root)).toEqual(['no suite: a.json']);
  });
});
