// The thresholds the SPA side of the cache-first client is written against
// (plan README, "Thresholds"). One definition each, in a module that imports
// nothing, so a surface can read a bound without importing the sync layer —
// which imports the app, which mounts the surfaces.
//
// The two remaining bounds in that table live where they are applied and
// nowhere else: the file-body ones in core/cacheLifetime.js, and the
// workspace-data TTL beside them.

/** Commits read when the cache holds no cursor to read forward from. */
export const LATEST_COMMITS = 20;

/** Conversation items read when the cache holds no sequence to read after. */
export const LATEST_THREAD_ITEMS = 100;

/** Unpushed commits whose patches are kept. */
export const UNPUSHED_COMMITS_MAX = 20;

/** The largest patch worth keeping, asked for on the wire and checked again
 *  on arrival — a bridge that ignored `max_bytes` must not put megabytes under
 *  a record this side says is 256 KB. */
export const COMMIT_PATCH_MAX_BYTES = 262144;

/** How long the background tier holds a flush before sending it. */
export const BACKGROUND_COOLDOWN_MS = 30000;

/** The record one commit's patch is kept under. */
export const PATCH_RECORD_KIND = "patch";
