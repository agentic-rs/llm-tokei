import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://agentic.tokn-ai.dev",
  base: "/llm-tokei",
  output: "static",
  trailingSlash: "always"
});
