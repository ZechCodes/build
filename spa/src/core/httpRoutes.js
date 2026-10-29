import { httpFailure } from "./httpEndpoint.js";

function validateRoute(route) {
  if (!route || typeof route.method !== "string" || typeof route.operationId !== "string") throw httpFailure("protocol");
  if (!new Set(["GET", "POST", "PUT", "PATCH", "DELETE"]).has(route.verb)) throw httpFailure("protocol");
  if (!/^\/v1\/[a-zA-Z0-9_{}./-]+$/.test(route.path) || route.path.includes("..")) throw httpFailure("protocol");
  const placeholders = [...route.path.matchAll(/\{([a-zA-Z0-9_]+)\}/g)].map((match) => match[1]);
  validatePlacement(route, placeholders);
}

function validatePlacement(route, placeholders) {
  if (JSON.stringify(placeholders) !== JSON.stringify(route.pathParams) || route.queryParams?.length || route.body !== "remaining") throw httpFailure("protocol");
}

export function createHttpRoutes(manifest) {
  if (manifest?.version !== 1 || !Array.isArray(manifest.operations)) throw httpFailure("protocol");
  const methods = new Map();
  const ids = new Set();
  for (const route of manifest.operations) {
    validateRoute(route);
    if (methods.has(route.method) || ids.has(route.operationId)) throw httpFailure("protocol");
    methods.set(route.method, Object.freeze({ ...route, pathParams: [...route.pathParams] }));
    ids.add(route.operationId);
  }
  return {
    operationIds: () => [...ids],
    has: (method) => methods.has(method),
    request(method, params = {}) {
      const route = methods.get(method);
      if (!route) throw httpFailure("unsupported");
      const body = { ...params };
      const path = route.path.replace(/\{([^}]+)\}/g, (_, key) => {
        const value = body[key];
        if (!["string", "number"].includes(typeof value) || value === "" || value === "." || value === "..") throw httpFailure("protocol");
        delete body[key];
        return encodeURIComponent(value);
      });
      if (route.verb === "GET" && Object.keys(body).length) throw httpFailure("protocol");
      return { operationId: route.operationId, method: route.verb, path, ...(route.verb === "GET" ? {} : { body }),
        ...(route.requestMaxBytes ? { requestMaxBytes: route.requestMaxBytes } : {}),
        ...(route.responseMaxBytes ? { responseMaxBytes: route.responseMaxBytes } : {}),
      };
    },
  };
}
