import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { listVectorFiles, loadVectorFile, REPO_ROOT, VECTORS_DIR, type VectorSource } from './schema.ts';

const NOTICES_FILE = join(REPO_ROOT, 'THIRD_PARTY_NOTICES');
/** Our own vectors carry our licence and need no third-party notice. */
const OWN_PROJECT = 'blazehttp';
const RULE = '='.repeat(80);

/**
 * Licence texts by SPDX id, only for licences whose text names no project or organisation; the copyright line comes
 * from each vector file's source.
 */
export const LICENCE_TEXTS: ReadonlyMap<string, string> = new Map(
  Object.entries({
    MIT: `Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the
following conditions:

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
USE OR OTHER DEALINGS IN THE SOFTWARE.`,
    /** curl's licence (`COPYING`), the text after its copyright line. */
    curl: `All rights reserved.

Permission to use, copy, modify, and distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright
notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF THIRD PARTY RIGHTS. IN
NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE
OR OTHER DEALINGS IN THE SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall not
be used in advertising or otherwise to promote the sale, use or other dealings
in this Software without prior written authorization of the copyright holder.`,
  }),
);

/** The Go project's BSD-3-Clause text after its copyright line, verbatim from `LICENSE` at the recorded commit. */
const GO_BSD_3_CLAUSE = {
  licence: 'BSD-3-Clause',
  text: `Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google LLC nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.`,
} as const;

/**
 * Licence texts that name an organisation (e.g. BSD-3-Clause's "Neither the name of …" clause), keyed by project and
 * used only when the source's licence matches. Taken verbatim from the licence file at the recorded commit; Go and
 * golang.org/x/net ship byte-identical licence files.
 */
const PROJECT_LICENCE_TEXTS: ReadonlyMap<string, { readonly licence: string; readonly text: string }> = new Map(
  Object.entries({
    go: GO_BSD_3_CLAUSE,
    'golang.org/x/net': GO_BSD_3_CLAUSE,
  }),
);

/** The licence text for `source`: its project's own text when it has one, else the generic SPDX text. */
export function licenceText(source: VectorSource): string {
  const own = PROJECT_LICENCE_TEXTS.get(source.project);
  const text = own?.licence === source.licence ? own.text : LICENCE_TEXTS.get(source.licence);
  if (text === undefined) throw new Error(`no licence text for ${source.licence} (${source.project})`);
  return text;
}

const HEADER = `Third-party notices

blazehttp ships no third-party code (design §6.1). The test vectors listed
below are adapted from the named projects and keep their notices.

Generated by tools/vectors/notices.ts from the vector files. Do not edit;
run pnpm vectors:notices.`;

export interface NoticeInput {
  /** Vector file path relative to the vectors root. */
  readonly path: string;
  readonly source: VectorSource;
}

/** One block per distinct (project, licence, copyright), listing every vector file it covers. */
export function renderNotices(files: readonly NoticeInput[]): string {
  const blocks = new Map<string, { source: VectorSource; lines: string[] }>();
  for (const { path, source } of files) {
    if (source.project === OWN_PROJECT) continue;
    const key = JSON.stringify([source.project, source.licence, source.copyright]);
    const block = blocks.get(key) ?? { source, lines: [] };
    block.lines.push(`  ${VECTORS_DIR}/${path} (from ${source.url})`);
    blocks.set(key, block);
  }
  const rendered = [...blocks.values()].map(({ source, lines }) => {
    const text = licenceText(source);
    return [
      RULE,
      `Project: ${source.project}`,
      `Licence: ${source.licence}`,
      '',
      'Files:',
      ...lines.sort(),
      '',
      source.copyright,
      '',
      text,
    ].join('\n');
  });
  return `${[HEADER, ...rendered].join('\n\n')}\n`;
}

/* v8 ignore start -- CLI wrapper, exercised by `pnpm vectors:notices --check` in CI */
if (import.meta.main) {
  const root = join(REPO_ROOT, VECTORS_DIR);
  const files = listVectorFiles(root).map((path) => ({
    path,
    source: loadVectorFile(join(root, path)).source,
  }));
  const text = renderNotices(files);
  if (process.argv.includes('--check')) {
    const current = readFileSync(NOTICES_FILE, 'utf8');
    if (current !== text) console.error(`THIRD_PARTY_NOTICES is out of date; run pnpm vectors:notices`);
    process.exitCode = current === text ? 0 : 1;
  } else {
    writeFileSync(NOTICES_FILE, text);
  }
}
/* v8 ignore stop */
