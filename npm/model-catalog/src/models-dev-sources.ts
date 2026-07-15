export type ModelsDevSource = {
  provider: string;
  model_prefix?: string;
};

/**
 * Curated models.dev sources for each model maker.
 *
 * A source may be a host when that is where models.dev publishes a vendor's
 * models. In that case `model_prefix` keeps the audit restricted to that
 * vendor's model records rather than every model sold by the host.
 */
export const modelsDevSourcesByVendor = {
  alibaba: [{ provider: "alibaba" }],
  anthropic: [{ provider: "anthropic" }],
  cohere: [{ provider: "cohere" }],
  deepseek: [{ provider: "deepseek" }],
  google: [{ provider: "google" }],
  meta: [{ provider: "meta" }, { provider: "llama" }],
  microsoft: [
    { provider: "azure", model_prefix: "phi-" },
    { provider: "azure-cognitive-services", model_prefix: "phi-" },
    { provider: "github-models", model_prefix: "microsoft/phi-" }
  ],
  minimax: [{ provider: "minimax" }],
  mistral: [{ provider: "mistral" }],
  moonshotai: [{ provider: "moonshotai" }, { provider: "moonshotai-cn" }],
  openai: [{ provider: "openai" }],
  xai: [{ provider: "xai" }],
  zai: [{ provider: "zai" }]
} as const satisfies Record<string, readonly ModelsDevSource[]>;

export type ModelsDevCatalogSource = ModelsDevSource & {
  vendor: string;
};

export function listModelsDevSources(): ModelsDevCatalogSource[] {
  return Object.entries(modelsDevSourcesByVendor)
    .flatMap(([vendor, sources]) => sources.map((source) => ({ vendor, ...source })))
    .sort((left, right) => {
      const provider = left.provider.localeCompare(right.provider);
      return provider || left.vendor.localeCompare(right.vendor);
    });
}
