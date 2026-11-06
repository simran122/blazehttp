import eslint from '@eslint/js';
import sonarjs from 'eslint-plugin-sonarjs';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  { ignores: ['**/node_modules/**', '**/dist/**', '**/coverage/**', '**/target/**', 'spike/**'] },
  eslint.configs.recommended,
  tseslint.configs.strictTypeChecked,
  {
    languageOptions: { parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname } },
    plugins: { sonarjs },
    rules: {
      '@typescript-eslint/no-deprecated': 'error',
      '@typescript-eslint/no-unused-vars': 'error',
      '@typescript-eslint/no-unused-expressions': 'error',
      'no-unused-vars': 'off',
      'no-unused-expressions': 'off',
      'sonarjs/no-commented-code': 'error',
      'no-restricted-syntax': [
        'error',
        { selector: 'SwitchStatement', message: 'Use a lookup map instead of switch.' },
      ],
    },
  },
  { files: ['**/*.js'], extends: [tseslint.configs.disableTypeChecked] },
);
