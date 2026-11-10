const newestById = (lists) => {
  const byId = new Map();
  for (const entry of lists.flat()) {
    const held = byId.get(entry.secret_id);
    if (!held || entry.revision > held.revision) byId.set(entry.secret_id, entry);
  }
  return [...byId.values()];
};

/** Own one conversation's async metadata reads and invalidate them on rebind. */
export function createSecretMetadataBinding({ cache, onPublish }) {
  let generation = 0;
  let stops = [];

  const release = () => {
    generation += 1;
    stops.forEach((stop) => stop());
    stops = [];
  };

  const bind = (scopes = []) => {
    release();
    if (!scopes.length || typeof cache?.read !== "function" || typeof cache?.subscribe !== "function") return;
    const boundGeneration = generation;
    const publish = async () => {
      const lists = await Promise.all(scopes.map((scope) => cache.read(scope)));
      if (boundGeneration !== generation) return;
      onPublish(newestById(lists));
    };
    const requestPublish = () => void publish().catch(() => {});
    stops = scopes.flatMap((scope) => [cache.subscribe(scope, requestPublish), ...(cache.track ? [cache.track(scope)] : [])]);
    requestPublish();
  };

  return Object.freeze({ bind, release });
}
