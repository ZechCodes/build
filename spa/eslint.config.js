// The SPA's only lint rule: cyclomatic complexity, capped at 10 per function
// (CLAUDE.md "Complexity gates"). No preset, no plugins — style is not the
// point; a function nobody can hold in their head is.
export default [
  {
    files: ["src/**/*.js"],
    languageOptions: { ecmaVersion: 2024, sourceType: "module" },
    rules: { complexity: ["error", 10] },
  },
];
