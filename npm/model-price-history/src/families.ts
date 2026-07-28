import { resolveModel } from "@tokn-ai/model-catalog";

import { getHistoricalModelRouteSnapshot } from "./history.js";
import type { ModelFamilyMapping, ModelFamilySnapshot, RepositoryOptions } from "./types.js";

export function getModelFamilySnapshot(options: RepositoryOptions): ModelFamilySnapshot {
  const snapshot = getHistoricalModelRouteSnapshot(options);
  return {
    commit_sha: snapshot.commit_sha,
    mappings: snapshot.routes.map((route) => mappingFromRoute(route))
  };
}

function mappingFromRoute(route: {
  provider: string;
  model: string;
  release_date?: string;
}): ModelFamilyMapping {
  const resolution = resolveModel({
    provider: route.provider,
    model: route.model
  });
  return {
    provider: route.provider,
    model: route.model,
    ...(resolution.canonical_name === null
      ? {}
      : {
          canonical_name: resolution.canonical_name,
          family: resolution.family
        }),
    ...(route.release_date ? { release_date: route.release_date } : {})
  };
}
