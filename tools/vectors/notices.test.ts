import { describe, expect, it } from 'vitest';
import { renderNotices } from './notices.ts';
import type { VectorSource } from './schema.ts';

const llhttp = (file: string): VectorSource => ({
  project: 'llhttp',
  url: `https://github.com/nodejs/llhttp/blob/abc/test/response/${file}.md`,
  commit: 'abc',
  licence: 'MIT',
  copyright: 'Copyright © 2018 Fedor Indutny',
});

describe('renderNotices', () => {
  it('one notice block per distinct (project, licence, copyright)', () => {
    const other: VectorSource = { ...llhttp('x'), copyright: 'Copyright (c) 2020 Someone Else' };
    const own: VectorSource = { ...llhttp('y'), project: 'blazehttp', licence: 'MIT OR Apache-2.0' };
    const text = renderNotices([
      { path: 'h1/llhttp-b.json', source: llhttp('b') },
      { path: 'h1/llhttp-a.json', source: llhttp('a') },
      { path: 'h1/other.json', source: other },
      { path: 'h1/own.json', source: own },
    ]);
    expect(text.match(/^Project: /gm)).toHaveLength(2);
    expect(text.match(/Copyright © 2018 Fedor Indutny/g)).toHaveLength(1);
    expect(text).toContain('Copyright (c) 2020 Someone Else');
    expect(text).not.toContain('own.json');
    expect(text.indexOf('llhttp-a.json')).toBeLessThan(text.indexOf('llhttp-b.json'));
    expect(text).toContain('Permission is hereby granted');
  });

  it('rejects a licence without a known text', () => {
    expect(() => renderNotices([{ path: 'x.json', source: { ...llhttp('x'), licence: 'GPL-3.0' } }])).toThrow(
      /GPL-3.0/,
    );
  });
});
