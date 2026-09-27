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

/** The newest items of a held conversation read again when something under
 *  the cursor changed while nobody was listening (#120): a delivery status, a
 *  settled message, a tool call's answer. A change further up than this is
 *  left as it was; the reader scrolling there reads it fresh. */
export const REPAIRED_THREAD_ITEMS = 50;

/** Unpushed commits whose patches are kept. */
export const UNPUSHED_COMMITS_MAX = 20;

/** The largest patch one record keeps, asked for on the wire and checked again
 *  on arrival — a bridge that ignored `max_bytes` must not put megabytes under
 *  a record this side says is 256 KB. A commit the reader opens past it is
 *  kept in pages beside a head record (#95, core/bodyPages.js). */
export const COMMIT_PATCH_MAX_BYTES = 262144;

/** The largest aggregate working-tree patch the `diff` record holds. Past it
 *  the record keeps the diff's shape alone, and each file's hunks are kept on
 *  their own, in pages where one is too large for a record (#95). */
export const WORKING_DIFF_MAX_BYTES = 1_048_576;

/** How long the background tier holds a flush before sending it. */
export const BACKGROUND_COOLDOWN_MS = 30000;

/** The record one commit's patch is kept under. */
export const PATCH_RECORD_KIND = "patch";

/** The record one task attachment's bytes are kept under, by path. */
export const ATTACHMENT_RECORD_KIND = "attachment";

/** The largest attachment body kept as one record: one bridge read's worth,
 *  which is every file a user can upload. An agent's longer recording is kept
 *  as page records instead (#95, core/bodyPages.js). */
export const ATTACHMENT_BODY_MAX_BYTES = 5 * 1_048_576;
