import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    coverage: {
      provider: 'v8',
      include: ['tools/**/*.ts'],
      exclude: ['**/*.test.ts', '**/check.ts', '**/npm-deprecated.ts', '**/generated/**'],
      thresholds: { lines: 95, branches: 95, functions: 95, statements: 95 },
    },
    projects: [
      {
        test: {
          name: 'tools',
          include: ['tools/**/*.test.ts'],
          execArgv: ['--throw-deprecation'],
        },
      },
      {
        test: {
          name: 'blazehttp',
          include: ['packages/blazehttp/test/**/*.test.ts'],
          execArgv: ['--throw-deprecation'],
        },
      },
    ],
  },
});
