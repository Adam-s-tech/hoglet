// Lint: typescript-eslint (recommended) + the React hooks rules. The generated
// contract types (src/types) and the build output are not ours to lint.
import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "src/types", "vendor", "node_modules"] },
  js.configs.recommended,
  tseslint.configs.recommended,
  {
    plugins: { "react-hooks": reactHooks },
    rules: {
      // The classic pair. The compiler-era rules (refs, set-state-in-effect) flag deliberate patterns here.
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],
    },
  },
);
