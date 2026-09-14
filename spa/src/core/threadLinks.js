// Where a file reference in a conversation points.
//
// An agent writes a reference as the path it was working on — relative to the
// checkout it is working IN — and nothing on the wire says which surface owns
// that checkout. The conversation's context does: a workspace conversation is
// about one workspace's directories, a branch conversation about one branch.
//
// So the route is decided here, once, and both the chip's href and the click
// that follows it read the same answer. Without one place for it the two drift:
// the href says one file and the click opens another, which is exactly the bug
// a middle-click is supposed to be free of.

import { directoryId } from "./workspaceModel.js";

/** Every name a workspace directory can be mounted under, best first: what the
 *  workspace calls it, and the ids it is routed by for a record with no name. */
const mountNames = (directory) =>
  [directory.name, directory.mount, directory.source_id, directory.id].filter(Boolean);

/// Which directory of a workspace a path is written against, and the path
/// within it — or null when the path is the open directory's own.
///
/// Only a workspace of SEVERAL directories mounts them under their names: with
/// one, an agent's paths are written from that directory's root, so a first
/// segment that happens to match its name is a real folder inside it. A bare
/// mount name with nothing under it is left alone too — there is no file there
/// to open, and a route to the empty path is a route to nowhere.
function mountedDirectory(directories, path) {
  const mounted = Array.isArray(directories) ? directories : [];
  if (mounted.length < 2 || !path) return null;
  const [head, ...rest] = String(path).split("/");
  if (!rest.length) return null;
  const directory = mounted.find((entry) => mountNames(entry).includes(head));
  return directory ? { sourceId: directoryId(directory), path: rest.join("/") } : null;
}

/// A Files route standing in one file, on whichever surface `base` names.
///
/// The line is the reference's FIRST line: a reference spanning 8-12 is read
/// from its top, and the Files tab stands on one line.
function filesRoute(base, path, link) {
  const line = Number(link.line_start);
  return {
    ...base,
    tab: "files",
    ...(path ? { file: path } : null),
    ...(path && Number.isFinite(line) && line > 0 ? { line } : null),
  };
}

/// How each kind of conversation turns a file reference into a route. A kind
/// with no entry here — an issue, which is about no checkout — names none.
const SURFACE_ROUTES = Object.freeze({
  workspace: (link, context) => {
    if (!context.projectId || !context.workspaceId) return null;
    const mounted = mountedDirectory(context.directories, link.path);
    return filesRoute(
      {
        name: "workspace",
        deviceId: context.deviceId ?? null,
        projectId: context.projectId,
        workspaceId: context.workspaceId,
        sourceId: mounted ? mounted.sourceId : context.sourceId,
      },
      mounted ? mounted.path : link.path,
      link,
    );
  },
  branch: (link, context) => {
    if (!context.projectId || !context.branch) return null;
    return filesRoute(
      {
        name: "branch",
        deviceId: context.deviceId ?? null,
        projectId: context.projectId,
        branch: context.branch,
      },
      link.path,
      link,
    );
  },
});

/**
 * The route a file reference opens, or null when the conversation cannot name
 * one — a reference of another kind, or a conversation about no checkout.
 *
 * `context` is the conversation's own: `{ kind, deviceId, projectId,
 * workspaceId, sourceId, directories, branch }`, of which each kind reads the
 * parts its surface is routed by.
 */
export function fileLinkRoute(link, context) {
  if (!link || link.kind !== "file" || !context) return null;
  const route = SURFACE_ROUTES[context.kind];
  return (route && route(link, context)) || null;
}
