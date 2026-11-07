import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { checkDrift, generate } from './gen.ts';
import { parseSpec, type Spec } from './spec-types.ts';

const loadJson = (path: string): unknown => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
const mini = parseSpec(loadJson('./fixtures/mini-spec.json'));
const withErrors = (errors: Spec['errors']): Spec => ({ ...mini, errors });

describe('generate', () => {
  it('generates TS and Rust for the mini fixture', async () => {
    const { ts, rust } = generate(mini);
    await expect(ts).toMatchFileSnapshot('./fixtures/mini-spec.ts.snap');
    await expect(rust).toMatchFileSnapshot('./fixtures/mini-spec.rs.snap');
  });

  it('generates the real spec with all 15 codes in order', () => {
    const real = parseSpec(loadJson('../../policy/spec.json'));
    expect(real.errors).toHaveLength(15);
    expect(generate(real).ts).toContain('export type HttpErrorCode =');
  });

  it('rejects string fields on wire codes', () => {
    expect(() => generate(withErrors([{ code: 'SSRF_BLOCKED', fields: { address: 'string' } }]))).toThrow(
      /wire: false/,
    );
  });

  it('rejects duplicate codes and empty detail lists', () => {
    const a = { code: 'TIMEOUT', details: ['connect'] };
    expect(() => generate(withErrors([a, a]))).toThrow(/duplicate code/);
    expect(() => generate(withErrors([{ code: 'TIMEOUT', details: [] }]))).toThrow(/empty/);
  });

  it('rejects malformed shapes', () => {
    const bad: Spec['errors'][number][] = [
      { code: 'lower', details: ['a'] },
      { code: 'A' },
      { code: 'A', details: ['a'], fields: { n: 'u32' } },
      { code: 'A', details: ['a', 'a'] },
      { code: 'A', details: ['a-b', 'a/b'] },
      { code: 'A', fields: {} },
      { code: 'A', fields: { x: [] } },
      { code: 'A', fields: { x: 'u32', y: 'u32' } },
      { code: 'A', fields: { x: ['a'], y: ['b'] } },
    ];
    for (const e of bad) expect(() => generate(withErrors([e])), e.code).toThrow();
    const many = Array.from({ length: 257 }, (_, i) => ({ code: `C${'A'.repeat(i + 1)}`, details: ['a'] }));
    expect(() => generate(withErrors(many))).toThrow(/256/);
  });

  it('rejects bad methods and limits', () => {
    expect(() => generate({ ...mini, methods: [] })).toThrow(/methods/);
    expect(() => generate({ ...mini, methods: ['CONNECT'] })).toThrow(/CONNECT/);
    expect(() => generate({ ...mini, methods: ['GET', 'GET'] })).toThrow(/duplicate/);
    expect(() => generate({ ...mini, limits: { h1: { maxHeadBytes: -1 } } })).toThrow(/limits/);
    expect(() => generate({ ...mini, limits: { h1: { max_head: 1 } } })).toThrow(/limits/);
  });
});

describe('parseSpec', () => {
  const base = loadJson('./fixtures/mini-spec.json') as Record<string, unknown>;
  const rejects = (raw: unknown, path: RegExp): void => {
    expect(() => parseSpec(raw)).toThrow(path);
  };

  it('names the JSON path of the first structural error', () => {
    rejects(null, /\$: expected an object/);
    rejects({ ...base, errors: 1 }, /\$\.errors: expected an array/);
    rejects({ ...base, errors: [{ code: 1 }] }, /\$\.errors\[0\]\.code: expected a string/);
    rejects({ ...base, errors: [{ code: 'A', wire: 'no' }] }, /\$\.errors\[0\]\.wire: expected a boolean/);
    rejects({ ...base, errors: [{ code: 'A', wires: false }] }, /\$\.errors\[0\]\.wires: unknown key/);
    rejects({ ...base, errors: [{ code: 'A', details: [1] }] }, /\$\.errors\[0\]\.details\[0\]/);
    rejects({ ...base, errors: [{ code: 'A', fields: { x: 'f64' } }] }, /\$\.errors\[0\]\.fields\.x/);
    rejects({ ...base, limits: { h1: { a: 'x' } } }, /\$\.limits\.h1\.a: expected a number/);
    rejects({ ...base, methods: [1] }, /\$\.methods\[0\]/);
  });

  it('accepts optional enumType', () => {
    expect(parseSpec(base).errors[2]?.enumType).toBe('QueueScope');
  });
});

describe('checkDrift', () => {
  it('reports drift', () => {
    const files = [
      { path: 'a.ts', content: 'one' },
      { path: 'b.rs', content: 'two' },
    ];
    const committed: Record<string, string> = { 'a.ts': 'one', 'b.rs': 'two' };
    expect(checkDrift(files, (p) => committed[p])).toEqual({ ok: true });
    committed['b.rs'] = 'modified';
    expect(checkDrift(files, (p) => committed[p])).toEqual({ ok: false, file: 'b.rs' });
    expect(checkDrift(files, () => undefined)).toEqual({ ok: false, file: 'a.ts' });
  });
});
