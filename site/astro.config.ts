import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://agentic-rs.github.io",
  base: "/llm-tokei",
  output: "static",
  trailingSlash: "always"
});
