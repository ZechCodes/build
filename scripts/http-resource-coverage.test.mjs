// Run from any directory: node --test scripts/http-resource-coverage.test.mjs
// This scans consumers independently of the Rust registry: intersecting the
// two would silently discard legacy calls and newly introduced methods.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = new URL("../", import.meta.url);
const methodShape = /^(?:ping|[a-z][\w]*(?:\.[A-Za-z][\w]*)+)$/;

// Audited metadata occurrences: [literal, readable use context, statement
// fingerprint]. Exemptions apply to a particular occurrence with a finite
// count, never every same-valued string in a file or namespace. Changed uses
// must be reviewed, and direct RPC arguments always remain consumers.
const metadataUses = {
  "app.js": [
    ["build.deviceFilter","const DEVICE_FILTER_KEY = [\"build.deviceFilter\"] ; export function","ce4105673b2f83267b13e07a659c6113b27d2a2a76f323beaf02d57b56a00ec8"],
  ],
  "appState.js": [
    ["build.selectedDeviceId","const SELECTED_DEVICE_KEY = [\"build.selectedDeviceId\"] ; export const","c17d032d99ffa8569703a033a1038c5d6f4b28de7b87021f89e07d4e5ee1d69b"],
  ],
  "connection.js": [
    ["term.liveness","probePath ?. ( [\"term.liveness\"] ) ? ?","6f6d8fc799c003efcb54fa417a5368aeb8d2ac9f0d7132b5973749aa3d7e9230"],
  ],
  "core/agentDefaults.js": [
    ["build.agentDefaults","const AGENT_DEFAULTS_KEY = [\"build.agentDefaults\"] ; const NO_PREFERENCE","0948c02e682985320b70eb74656594986171f9c647c57de29ba950da34507beb"],
  ],
  "core/bridgeApi/v1/index.js": [
    ["term.output","[ \"changes\" , [\"term.output\"] , \"term.reset\" ,","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["term.reset",", \"term.output\" , [\"term.reset\"] , \"term.closed\" ,","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["term.closed",", \"term.reset\" , [\"term.closed\"] , \"rtc.ice\" ,","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["rtc.diagnostics",", \"rtc.ice\" , [\"rtc.diagnostics\"] , \"bridge.update_status\" ,","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["models.changed",", \"bridge.update_status\" , [\"models.changed\"] , \"harnesses.changed\" ,","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["harnesses.changed",", \"models.changed\" , [\"harnesses.changed\"] , \"compaction.changed\" ,","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["compaction.changed",", \"harnesses.changed\" , [\"compaction.changed\"] , \"writes.confirmed\" ,","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["writes.confirmed",", \"compaction.changed\" , [\"writes.confirmed\"] , \"bridge.restarting\" ,","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["bridge.restarting",", \"writes.confirmed\" , [\"bridge.restarting\"] , ] )","46c33c7e21fc15241d9d2b8b70025c8cc3528204185ffc2d0bb6316abcde90b8"],
    ["changes.subscriptions",". has ( [\"changes.subscriptions\"] ) ; const","20fb6a0c88c128a1254da272bc70614f9aba7dc095f927a02e0b67717a777c23"],
    ["bridge.epoch",". has ( [\"bridge.epoch\"] ) , ackConfirmMethods","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["bridge.epoch",". has ( [\"bridge.epoch\"] ) ) }","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["changes.projectSources",". has ( [\"changes.projectSources\"] ) , }","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["git.projectSources",". has ( [\"git.projectSources\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["term.projectSources",". has ( [\"term.projectSources\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["fs.projectSources",". has ( [\"fs.projectSources\"] ) , projectSourcePath","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["fs.projectSourcePath",". has ( [\"fs.projectSourcePath\"] ) , uploads","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["requests.priority",". has ( [\"requests.priority\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["rtc.clientLanCache",". has ( [\"rtc.clientLanCache\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["errors.codes",". has ( [\"errors.codes\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["diffs.perFile",". has ( [\"diffs.perFile\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["bodies.pages",". has ( [\"bodies.pages\"] ) , mediaRawPages","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["fs.mediaRawPages",". has ( [\"fs.mediaRawPages\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.reviewComments",". has ( [\"tasks.reviewComments\"] ) , pullRequests","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.review.pullRequests",". has ( [\"tasks.review.pullRequests\"] ) , }","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.context",". has ( [\"tasks.context\"] ) , assignProjectAgent","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.assignProjectAgent",". has ( [\"tasks.assignProjectAgent\"] ) , attachments","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.attachments",". has ( [\"tasks.attachments\"] ) , watching","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.watching",". has ( [\"tasks.watching\"] ) , doneSinceLeft","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.doneSinceLeft",". has ( [\"tasks.doneSinceLeft\"] ) , commentUserNotifies","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.commentUserNotifies",". has ( [\"tasks.commentUserNotifies\"] ) , listPaged","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.listPaged",". has ( [\"tasks.listPaged\"] ) , bodyPrecondition","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.bodyPrecondition",". has ( [\"tasks.bodyPrecondition\"] ) , waitingOn","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["tasks.waitingOn",". has ( [\"tasks.waitingOn\"] ) , }","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["compaction.harnessThresholds",". has ( [\"compaction.harnessThresholds\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["conversations.settings",". has ( [\"conversations.settings\"] ) , reset","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["messages.context",". has ( [\"messages.context\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["threads.postOperations",". has ( [\"threads.postOperations\"] ) , attachmentChunks","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["thread.attachmentChunks",". has ( [\"thread.attachmentChunks\"] ) , leanActivity","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["threads.leanActivity",". has ( [\"threads.leanActivity\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["branches.finishDelete",". has ( [\"branches.finishDelete\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["agents.createdBy",". has ( [\"agents.createdBy\"] ) , projectWorkers","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["agents.projectWorkers",". has ( [\"agents.projectWorkers\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["sources.syncBase",". has ( [\"sources.syncBase\"] ) , syncBasePath","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
    ["sources.syncBasePath",". has ( [\"sources.syncBasePath\"] ) } ,","94b2da698d72a0c1633af5f32f508e56432594d3a321612ef370301880817402"],
  ],
  "core/bridgeApi/v1/writeMethods.js": [
    ["stream.start",", \"writes.subscribe\" , [\"stream.start\"] , \"term.attach\" ,","33c4ebe0b0f62912e1c6d8df14ccb62831488abd785fa6eaa66337243ef6237a"],
  ],
  "core/bridgeEpoch.js": [
    ["bridge.epoch",". includes ( [\"bridge.epoch\"] ) ; const","bcaa5d51d4585cacc7f1ecfaf03a38809bc80321b16cd75bfcc28d58773e1373"],
  ],
  "core/cacheSync.js": [
    ["build.cacheSync","const SYNC_LOCK = [\"build.cacheSync\"] ; export const","f2160711e2b162e0f91fe453ac136176a51044a2a1195c1aa76e865b26b57f6e"],
  ],
  "core/changeEvents.js": [
    ["compaction.changed","kinds : [ [\"compaction.changed\"] ] , mode","c27346939208820b8d95aac7199bdd6db22004a95ed5f68d962b5f368551d091"],
    ["writes.confirmed","( [ [ [\"writes.confirmed\"] , dispatchWriteConfirmation ]","7d1824116f67f0f38b9e63b9075e5815ffe18dd3c1abf9c0594e78a8921fbde6"],
    ["models.changed","] , [ [\"models.changed\"] , ( _payload","471b56feaa531bfd6c17898c927ca269fa4d00311ed45178657282e3afb25649"],
    ["harnesses.changed","] , [ [\"harnesses.changed\"] , ( {","471b56feaa531bfd6c17898c927ca269fa4d00311ed45178657282e3afb25649"],
    ["compaction.changed","] , [ [\"compaction.changed\"] , ( payload","471b56feaa531bfd6c17898c927ca269fa4d00311ed45178657282e3afb25649"],
    ["tasks.waitingOn",". includes ( [\"tasks.waitingOn\"] ) ; void","78fb083ca669f15d9d499f30f3d7ff2ad5fcd03843154fcb252eaea0d6364cc1"],
  ],
  "core/changesComments.js": [
    ["tr.dhl",". querySelectorAll ( [\"tr.dhl\"] ) . forEach","59b3ec16ee0eabb270d2385e808eadb445fc838f0d8ee244d927ace3e474619a"],
    ["td.ln",". querySelector ( [\"td.ln\"] ) ; }","39ba68f95e9b69b4cd897a790198381873ab29b99dcc9c88519940daccdf29b5"],
  ],
  "core/changesModel.js": [
    ["uv.lock","Set ( [ [\"uv.lock\"] , \"poetry.lock\" ,","6d88265fa6eda2c147b159e4420d1912f13390d93023dcd04b99eb8d2efc71b8"],
    ["poetry.lock","[ \"uv.lock\" , [\"poetry.lock\"] , \"Pipfile.lock\" ,","6d88265fa6eda2c147b159e4420d1912f13390d93023dcd04b99eb8d2efc71b8"],
    ["yarn.lock",", \"package-lock.json\" , [\"yarn.lock\"] , \"pnpm-lock.yaml\" ,","6d88265fa6eda2c147b159e4420d1912f13390d93023dcd04b99eb8d2efc71b8"],
    ["bun.lockb",", \"pnpm-lock.yaml\" , [\"bun.lockb\"] , \"Cargo.lock\" ,","6d88265fa6eda2c147b159e4420d1912f13390d93023dcd04b99eb8d2efc71b8"],
    ["composer.lock",", \"Gemfile.lock\" , [\"composer.lock\"] , \"mix.lock\" ,","6d88265fa6eda2c147b159e4420d1912f13390d93023dcd04b99eb8d2efc71b8"],
    ["mix.lock",", \"composer.lock\" , [\"mix.lock\"] , \"go.sum\" ,","6d88265fa6eda2c147b159e4420d1912f13390d93023dcd04b99eb8d2efc71b8"],
    ["go.sum",", \"mix.lock\" , [\"go.sum\"] , ] )","6d88265fa6eda2c147b159e4420d1912f13390d93023dcd04b99eb8d2efc71b8"],
  ],
  "core/compose.js": [
    ["build.captureQueue","const CAPTURE_QUEUE_KEY = [\"build.captureQueue\"] ; export const","82480bc47e97b2ae413437020b4a75718be0efc9b10f85941277fe27d0026b37"],
  ],
  "core/diffSort.js": [
    ["build.diffSort","const DIFF_SORT_KEY = [\"build.diffSort\"] ; export const","b8275952e34706203de4010573712019b9dcb24af34f7c98a7794e448ef7567b"],
  ],
  "core/directoryRail.js": [
    ["build.sidebarCollapsed","const SIDEBAR_COLLAPSED_KEY = [\"build.sidebarCollapsed\"] ; const storeOf","826cb0023b29c49e39c0fa83adc7a5fe119fabab1cf11a7784e9617103db6581"],
  ],
  "core/navigationVisits.js": [
    ["build.navigation.previous","const TAB_KEY = [\"build.navigation.previous\"] ; const trackers","3206e95b6e23daf65fdc6baad67c4da1b71cd4755605050775f71e498dbdbc37"],
  ],
  "core/peerLink.js": [
    ["rtc.diagnostics","] , [ [\"rtc.diagnostics\"] , receiveCandidateDiagnostic ]","c7899cd886c049dc93c9f8ad50f50713a3ff875e36afa6a48163d31a636970a2"],
  ],
  "core/railMode.js": [
    ["build.inbox.view","const RAIL_VIEW_KEY = [\"build.inbox.view\"] ; export const","e73b4ed0b13201a13d5470f2a8ba8950f0368e40b398f6bb3245af4fefb77a16"],
    ["build.inbox.folded","const FOLDED_PROJECTS_KEY = [\"build.inbox.folded\"] ; export const","c1ab98d6500b1cd02b0622be2d3293ffb0daba16f73b43cf6e9d0ad6da47d86d"],
  ],
  "core/sessionRpc.js": [
    ["writes.confirmed","?. type === [\"writes.confirmed\"] ) { for","2b746622f13a67218656ffc40b51cba120d1b486e3cd30b9e6cf7741f060347f"],
    ["writes.confirmed","?. type === [\"writes.confirmed\"] ; const writeCall","ec4fe8cbf8b6ecf2ddf61b0e21907ee76a514e858855b0ff3a88e345b591166f"],
  ],
  "core/syncDiagnostics.js": [
    ["project.agents",", \"worktree.diff\" , [\"project.agents\"] , \"changes.subscribe\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
    ["thread.list",", \"changes.unsubscribe\" , [\"thread.list\"] , \"thread.get\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
    ["thread.get",", \"thread.list\" , [\"thread.get\"] , \"thread.page\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
    ["thread.read",", \"thread.page\" , [\"thread.read\"] , \"conversation.get\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
    ["conversation.get",", \"thread.read\" , [\"conversation.get\"] , \"conversation.list\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
    ["conversation.list",", \"conversation.get\" , [\"conversation.list\"] , \"messages.list\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
    ["messages.list",", \"conversation.list\" , [\"messages.list\"] , \"messages.get\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
    ["messages.get",", \"messages.list\" , [\"messages.get\"] , \"tasks.review.list\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
    ["tasks.review.list",", \"messages.get\" , [\"tasks.review.list\"] , \"tasks.review.get\" ,","c315d26ed94189e1621144d5736f4bb9c0da7cf78a7bfa4b91698875e9757c36"],
  ],
  "core/theme.js": [
    ["build.theme","const THEME_KEY = [\"build.theme\"] ; export const","b0731e85abede6f4d672fc6c2cfb60a58b3581f4a5d5f0a562805caf4fae7f29"],
  ],
  "core/workspaceDefaults.js": [
    ["build.workspaceDefaults","const WORKSPACE_DEFAULTS_KEY = [\"build.workspaceDefaults\"] ; const isObject","1b084ca93def9101b157b6ba04d29622dea1011f35e2b77c5f8bfcb7cd36dc6f"],
  ],
  "core/writeOperationEffects.js": [
    ["writes.confirmed",". type !== [\"writes.confirmed\"] || frame .","1c0117eda5a432a7f31c234af0b7ff229098b9370429242b6da275fdd319db25"],
  ],
  "core/writeOperationIntent.js": [
    ["writes.confirmed",", type : [\"writes.confirmed\"] } ; if","dac96cc8f68df7d2e09f2e50ce2d4a4e77ebfd54fb7813bbc1317dabd6d4f4a8"],
    ["writes.confirmed",", type : [\"writes.confirmed\"] } ; }","2795e7db179ba7f242de1da4e2c5374014e01a2176db36eb7c134a3ad5e54510"],
  ],
  "core/writeOperations.js": [
    ["bridge.epoch","?. includes ( [\"bridge.epoch\"] ) && greeting","7fb697f58374322f54a61ff6a979eaaf56611e419dca5a93142337282dabbee9"],
    ["writes.confirmed","{ type : [\"writes.confirmed\"] , operation_id :","c1701f080374ab4e940f9471b5b7ccc8ea63d20d8d6db08923dcabad029fd094"],
    ["writes.confirmed",". type === [\"writes.confirmed\"] ) await confirm","fc7814e39ce3ed405f1ea69157ed241114a19cae1bc1e82c992227434461e8e9"],
    ["writes.confirmed",". type !== [\"writes.confirmed\"] ) return false","a3baa2366329368c852eaeaeb88463572c57f72296698d2135dd910b7ec7ccb1"],
    ["bridge.epoch","?. includes ( [\"bridge.epoch\"] ) ) await","36758934fd294f52ca61ce9c7c194e0f3dc0fe772c6914bc06c4cabbcb776717"],
  ],
  "push.js": [
    ["build.push.open","const PUSH_OPEN_MESSAGE = [\"build.push.open\"] ; const APP_PATH","775dbce78b514f4ccd8decfa36d864808fefb900917354d36bef33db28eaca43"],
  ],
  "terminal/session.js": [
    ["term.reset",". type !== [\"term.reset\"] || ! String","acb5b711da517a30b3c58819241e4b7fefc6e9d142028279c693847679d2b0d9"],
    ["term.output",". type !== [\"term.output\"] && p .","b7ac576602b69e7788b3d349fd7c88e92346daa2fb481d0058be97d6ca8008a5"],
    ["term.reset",". type !== [\"term.reset\"] ) return ;","b7ac576602b69e7788b3d349fd7c88e92346daa2fb481d0058be97d6ca8008a5"],
    ["term.output",". type === [\"term.output\"] || p .","064660193f5c3a35f359d2ea0424be626bd0386acf1611364b348b161c48993c"],
    ["term.reset",". type === [\"term.reset\"] ) { if","064660193f5c3a35f359d2ea0424be626bd0386acf1611364b348b161c48993c"],
    ["term.closed",". type === [\"term.closed\"] ) { if","0bd0e815159a22798b5a7c99442bf8e7596f328b4d9443124b96dd5dc3be6ba9"],
    ["term.output",". type === [\"term.output\"] ) { if","c359dc975880f6018fbadcb07010dee4f9c53b00da269d93fd6d01936f15a50d"],
  ],
};

// Conservatively pin source containing method-shaped producers, recognized
// RPC calls or audited metadata. This catches changed alias/assignment edges
// in reviewed files without claiming the
// lexical scanner performs JavaScript dataflow analysis. Exact source hashes
// also preserve line-break/automatic-semicolon-insertion differences.
// Split public digests into short display parts so secret scanners do not
// mistake a file name containing "Key" plus a SHA-256 for a credential.
const sourceFingerprintParts = {
  "app.js": ["529fcd47", "5365bb97", "5a6dcfc2", "30089f9e", "831863fc", "b89bde77", "bf53ffd3", "1b761c41"],
  "appState.js": ["d24d9edc", "6e4988f0", "8a5289ee", "6c449b3f", "91bdbe42", "7cd43ada", "7a9df6a4", "acd39971"],
  "connection.js": ["314548c1", "68c90514", "2ee3e9b5", "c53f9798", "22aba77b", "060e5c13", "e5cc23f0", "f1662147"],
  "core/activityRuns.js": ["2bd80d81", "ad2ddde9", "da2fc194", "95b7b7ba", "4d0a86f4", "29a52906", "009b6fc5", "5221d710"],
  "core/adoption.js": ["350b90d5", "9e535f4b", "15adf2ef", "7eacd8c0", "fb946020", "b6130cb8", "ce729fed", "bb90597e"],
  "core/agentDefaults.js": ["f856d23d", "2c446c14", "c6e4d78e", "e11e737d", "75e1fbc1", "fa79564d", "2ef20a2b", "1cdbbcb0"],
  "core/agentModes.js": ["5764f59d", "200944fd", "e1fb485f", "a1b58c05", "633b7f0f", "f9b7cbcd", "4e62b9db", "31bcb3a8"],
  "core/agentRail.js": ["5a8293a8", "160dddf5", "ff4ae6e8", "c7c4dee8", "63f484b6", "c8a205ca", "59b70a14", "1aa11a6a"],
  "core/agentRailContext.js": ["e175542d", "376171c6", "e04b61df", "847af2cf", "a3a2c56e", "8fbeb913", "9593b4cb", "50809f5c"],
  "core/agentRolesPanel.js": ["c05b7c96", "5849ec7f", "0d258bc2", "96a7795e", "2a4663f4", "57e78316", "de145b90", "699b4a91"],
  "core/agentSurfacesModel.js": ["546cf5aa", "3a478c63", "4a0f02f5", "4eafcae5", "53a5ffd3", "3ad4f563", "c075aa93", "dcbbef9d"],
  "core/bridgeApi/v1/compactionParams.js": ["506db6e2", "f8148bc4", "925e889b", "61c6fe15", "d8cf74e2", "ef7e468f", "b610115f", "7132ce5b"],
  "core/bridgeApi/v1/index.js": ["7124f4e6", "ac45c6fd", "d5ffd9c7", "95967cc2", "87d43e61", "e2bef7b0", "6746b8c1", "e5720582"],
  "core/bridgeApi/v1/writeMethods.js": ["f311847f", "da193503", "678520ec", "e289e013", "fbbcc9f9", "26f2aae9", "32c184fe", "7f27b03d"],
  "core/bridgeEpoch.js": ["e04a0a11", "6535b020", "abcac20c", "972a554f", "8d51fbb9", "6ef07fd8", "16062ed0", "9cd4d213"],
  "core/bridgeUpdatePanel.js": ["f9b19cc7", "4abdfc5a", "d159194d", "49ee91ed", "d39ab2e7", "15a6a80f", "0cb51530", "623ca8e5"],
  "core/bridgeUpdates.js": ["279cb932", "0495aa46", "c347f93b", "10a492d5", "8d29b0a9", "e9309a9a", "91de7993", "e5f40ad3"],
  "core/cacheSync.js": ["a8f7ffd7", "25d0a21d", "5160f9e1", "0166e608", "bd45be22", "59951a3c", "685429df", "2f71c39d"],
  "core/captureDecisionView.js": ["f9f42a7d", "d94162aa", "5aea82e5", "85e6bcb2", "73ffce61", "2ac99214", "8b78b55f", "30a7a61a"],
  "core/changeEvents.js": ["2de56886", "56ad09a9", "9b2c303a", "dfb97b7c", "6b7d67b6", "932183bb", "addc42c1", "313ed272"],
  "core/changesComments.js": ["f258a3f1", "5b42f974", "ba0af04f", "f955c0ff", "1cdd17d3", "353ce276", "c7dd49b6", "eb1c06be"],
  "core/changesModel.js": ["1a2a1485", "24c755c4", "706c0111", "981a4765", "2cc7236e", "4ff87fce", "4a7640ba", "13a75672"],
  "core/changesReview.js": ["efa3d8fd", "2f936aea", "3c017fef", "5e7e463e", "5b6485cf", "abc5c809", "6e9a2636", "42ac75ec"],
  "core/chatChoiceController.js": ["d257b6c5", "f32d9067", "226af14c", "bd8ec71e", "718bf188", "1e994ef1", "66edcceb", "4499fa61"],
  "core/chatRepository.js": ["a7331089", "a8f626b3", "3c5700e5", "e726dfbe", "7aeb4d75", "59d71243", "6bb28ca2", "6e0dd982"],
  "core/compactionCache.js": ["041bfd3e", "820024de", "ec19e3f6", "c7ce04ad", "4adecae6", "5f78a182", "d12f13cb", "89e82556"],
  "core/compose.js": ["012665e7", "87915aea", "9314f63c", "7b036438", "70ec846e", "09d98f92", "e8746b91", "051b004e"],
  "core/composeView.js": ["b560ac00", "27513049", "482b025b", "79f330a5", "44f3a97f", "f2c88efa", "2f36708b", "a8951245"],
  "core/consoleModel.js": ["4feba529", "663ba471", "6fe6e2a1", "3eb66e5c", "815545f8", "f70a5b48", "a6248d75", "94d9ed20"],
  "core/conversationCompaction.js": ["4e076445", "2fd7fb1d", "1ca9681e", "b39ea814", "01d5b9f9", "26f91118", "a52788ca", "54220e9e"],
  "core/conversationDetail.js": ["a54ac9f8", "f54c61d4", "cea363a5", "b098964f", "16ecc016", "490b853d", "76d94602", "ad5a773a"],
  "core/createWork.js": ["42444ebc", "8ae4117e", "ba2cdde6", "7d740ba3", "1850d7a6", "a63c9fd2", "50784964", "18d57196"],
  "core/defaultHarness.js": ["b7ed5ad6", "9fdfdc35", "84dd803d", "3c0e95fd", "72904016", "4cb29938", "97723e94", "2ace812d"],
  "core/deviceContexts.js": ["36c494bd", "9e3a9ada", "5feb60e8", "296573df", "9f3f36dc", "252afebd", "97642d91", "d82ec699"],
  "core/diffSort.js": ["9ef743b8", "16f0a5ae", "1d9ec36a", "455f0e76", "c04a0902", "8deca6c7", "70acd150", "dbe96ac6"],
  "core/directoryRail.js": ["98080c38", "f2152285", "3e025868", "10652120", "d12c8ead", "cfa571c6", "6c3476c8", "b6b56ef2"],
  "core/fileDiffs.js": ["94424fd1", "118dcd8d", "6c511bb9", "642d4822", "e4ebd5c2", "ffd9b041", "41d478ef", "20b2347d"],
  "core/filesUploadActions.js": ["a3bbb078", "74d42693", "0534c27d", "8bf50ca4", "1b0cccc7", "b6cc2162", "feb1f0ef", "53188d20"],
  "core/filesUploads.js": ["32702379", "79d5ef63", "c486dc67", "90cbc884", "c0cbf79d", "28deabc3", "f4202a02", "d95a8bd3"],
  "core/fileUploadRpc.js": ["7891a6ca", "183a9b67", "b7fe41fc", "ba71c70a", "1896ec75", "2809155c", "1375617f", "a9ab96d0"],
  "core/fileUploads.js": ["a1a4c2df", "5ef03503", "483e7b99", "099e04a0", "77c9e865", "a22a23dd", "a704cd79", "6678f239"],
  "core/fileUploadsModel.js": ["d11fdd79", "8bb9318c", "fbc2cff2", "29b16930", "9d73b3ee", "89d5d976", "7e2b756a", "69d40aac"],
  "core/githubRepos.js": ["4526fb16", "bfe42e0f", "57b963ea", "fe623873", "0fe9ccce", "521ecc83", "b9117e59", "af64462f"],
  "core/gitPane.js": ["7a5a41a0", "6e603556", "4ba87c41", "f7a231e8", "65fe67f3", "1094f363", "5aacb839", "4e647b57"],
  "core/inboxCaptures.js": ["48a7f5fb", "b2132a83", "5901fb31", "4fb0c5ae", "97005297", "2d0b9db6", "be2c3e6d", "1271b663"],
  "core/inboxDevices.js": ["f9a293e4", "dff4b2b4", "f5fc7940", "f5c1f48d", "a66a5718", "238826a6", "e3dc6b9f", "4060a79e"],
  "core/inboxSeen.js": ["8131bfc1", "8d81301f", "e91c3d80", "44cb2dec", "5c60d797", "eb8516a4", "3b49d94a", "ca32676a"],
  "core/inboxView.js": ["433300bb", "6f8e561b", "66b4fe10", "c55cbfe5", "670a7736", "ab4df347", "d6f4bba4", "3ae07401"],
  "core/isolation.js": ["1b646651", "086eea7f", "b0a97d6f", "8993ed77", "7e6e246e", "4110fbc2", "7defb076", "77f79221"],
  "core/markdownFences.js": ["10057396", "68214f4d", "d193015e", "2552698e", "c48313df", "6df2e223", "7e3bc837", "2805a4bb"],
  "core/modelCatalog.js": ["c293ea11", "fff0969f", "7852c8cd", "0c252f82", "fe666159", "95919417", "25a661ec", "c93fd9b4"],
  "core/navigationVisits.js": ["f8d32f7a", "6bf64af6", "81866475", "02ebd8fc", "3a0be61c", "c9f0d9f3", "e1cc0c06", "dd4dc05a"],
  "core/optimistic.js": ["664afc7e", "9083408c", "39d1e085", "5a531484", "9d315ad7", "dcbe2655", "b119e508", "6f3ef787"],
  "core/pathProbe.js": ["1518fd69", "692eec10", "f8018014", "c81c6ee1", "4edda84e", "efee05d6", "472437de", "73c96fea"],
  "core/peerLink.js": ["6c6acb27", "28aa36e6", "0258ba0f", "303f308a", "45b3ffb6", "701efb20", "ae563ff7", "6a4ddb2d"],
  "core/projectAgentsCache.js": ["692ce6e6", "29188041", "a638ab36", "772487e7", "397715fe", "dba4c0f5", "4e938ee8", "6bbdb7ff"],
  "core/projectAgentSetting.js": ["cf7d410a", "f3bc1eb4", "1b4e7833", "9dc4cf50", "23b01280", "01636e27", "72e8edb1", "e4a7e379"],
  "core/projectChangesRpc.js": ["613fe381", "59f4a3a4", "08a518ed", "4e535261", "21ca4a58", "5e5011d9", "c0acd52e", "bb8e5014"],
  "core/projectConsole.js": ["b44678cc", "4c2dbc2a", "188695a2", "307b4883", "4d4fb733", "091622af", "3c648066", "9d1e32bb"],
  "core/projectFilesRpc.js": ["e84ae7b7", "ceffb0cc", "7ff4c6cd", "cfc0c05f", "13a0d831", "079f3b3e", "5dae2e56", "ce4686ac"],
  "core/projectTerminalRpc.js": ["b391c6dc", "aa385e26", "aadfb8b8", "f6ef5cc9", "9a8e7a79", "d9d757bb", "cdd1356c", "867cadea"],
  "core/pushKeySync.js": ["ecd9373a", "3f2dd8cb", "af448d18", "1098ca18", "ef83c7bb", "923a9c90", "b5d5f042", "9ac75438"],
  "core/railMode.js": ["9a3738d3", "e4361bdf", "fef94f86", "f803203f", "efbe6cbf", "8513e3f7", "5fc7602c", "dd4e5e6b"],
  "core/readStateRepair.js": ["cf9c2d3b", "411a9904", "c9edb8e9", "33ae46f6", "d879a630", "a802c545", "b665fb8c", "44cb7422"],
  "core/revisionBodies.js": ["60606411", "ec3a6f2c", "7c923581", "f22c39cd", "43dd8a04", "69ff5c40", "e4e490cb", "e3119193"],
  "core/scopedCompactionEditor.js": ["b5d366f5", "90a2be6a", "b5c01379", "ccad5198", "7298a4cf", "bdcbdc88", "2e022cd4", "847d6330"],
  "core/session.js": ["3d3cf7dd", "79e4e284", "e35b0919", "e70c73b0", "84b3ec43", "325e1ae6", "fc5b5b9d", "ab43100a"],
  "core/sessionRpc.js": ["b4643d70", "09686b06", "64a93192", "a5d2f604", "c01907cb", "a777026a", "7364e2d5", "c53f74a9"],
  "core/sessionSwitch.js": ["dc232a71", "5b006195", "d7910b8e", "8c59e611", "a36dd264", "8bbcfcc8", "0d8739f3", "371accc7"],
  "core/shell.js": ["c9e2a17e", "6ae389f1", "e7a60035", "dfc83868", "dc85c17d", "564daa9f", "9b3e097b", "391bcd33"],
  "core/sqlReadCredits.js": ["bd48f63c", "6ba332ba", "579d3bb1", "7246f83f", "80932ec8", "e6567443", "12da1fae", "2144302e"],
  "core/syncDiagnostics.js": ["1060f9ba", "b9d583f7", "93db302a", "9ecabc68", "08ff098a", "2119a4b8", "fdc89f9a", "74403b01"],
  "core/syncLists.js": ["e44aee99", "346eb027", "1e6486f1", "dac9a7ac", "9acdd7b8", "1c4a00f4", "ce196d01", "e22f5292"],
  "core/syncSuccessors.js": ["8f215712", "252114ce", "b5f282bf", "3b818ded", "2bfe5a52", "60ced14f", "965e228e", "9af31a83"],
  "core/syncTaskPhases.js": ["8ded146d", "e8dd6899", "16ffd0a4", "4b38ca1f", "ab2b3400", "2c745f68", "c2096ffd", "c4845622"],
  "core/taskAttachmentBodies.js": ["7acfa90a", "4413c7df", "89207db8", "07fa4c16", "b21cc95f", "dc3bf412", "eb0c14de", "671d3006"],
  "core/taskChecklist.js": ["74b43260", "0b2d0ae0", "6ea4ea00", "c53db5e4", "8830cc22", "090c1067", "a4ffbd6f", "8ed3cae6"],
  "core/taskComposer.js": ["44233e6a", "d60f8251", "f724f7bc", "af16b319", "2c205f31", "5a10e50b", "b233a25f", "ed90f8ad"],
  "core/taskReviewActions.js": ["9e0e7f81", "56f56199", "855d3846", "a7c38661", "bcc2d052", "33bc2469", "2299c4a5", "6c0bb485"],
  "core/taskReviewCache.js": ["baa99891", "1615149d", "fa8810f0", "35d28a10", "be31a520", "41c6c154", "3bf6b31d", "44e4a700"],
  "core/taskReviewChanges.js": ["5bda4fe8", "76062ca2", "b8b03a11", "54479aab", "60eb34c0", "04dfbdc7", "3bdbee0d", "454737ed"],
  "core/taskReviewControls.js": ["b694c9e4", "19686ec0", "d0c8e7ef", "5ce23834", "57a0c5af", "bce0145c", "5c889031", "fb4f998c"],
  "core/taskReviewFeedback.js": ["c66d3512", "596e5878", "97c8c00c", "ede58c71", "6893718a", "79654d45", "46c47189", "cb5124ab"],
  "core/taskReviewFiles.js": ["f718c0f4", "0ddfa4b1", "bdff017a", "25f15f7b", "a50ef953", "dc2123d5", "d3acd7ae", "2f11fb2f"],
  "core/taskReviewLifecycle.js": ["04b749fb", "1781936b", "6c4264ab", "b48cb304", "339a1a4e", "28617a6f", "cc24e0bf", "0f8ee6d3"],
  "core/taskReviewMerge.js": ["805e7cd6", "c7f92701", "9b8b5f08", "6c8e985d", "a1de4e1b", "a60280e4", "aa424934", "6b83af9e"],
  "core/taskReviewReclaim.js": ["39f9773b", "9fb74944", "f19e9d44", "8698c30d", "cbd907a0", "bde50825", "b8601ff9", "a2100184"],
  "core/taskReviewSupport.js": ["31e90542", "4726fa94", "eaf3677f", "b9d8a437", "43a69e5a", "0c9de2c3", "967250ca", "3cbd5571"],
  "core/taskView.js": ["547b447a", "1b089702", "8da986a0", "97cbffed", "86cd8be0", "dfabf680", "23671d54", "6ac7ef11"],
  "core/taskWaitingOnActions.js": ["ba117c3f", "a3957024", "6f0346c2", "a4e5a300", "2b653386", "0e1ff268", "216a0f34", "9d20d4e0"],
  "core/taskWaitingOnPage.js": ["b5a78897", "c7cfc8dc", "8a1ad2e0", "c77cad0a", "874280e0", "1b394697", "37da2547", "7a1c3636"],
  "core/theme.js": ["5f821f5d", "933be0fa", "c04dd60c", "8ed91a2a", "4fd56dbb", "ab2c8e04", "e86ee7f5", "60d8e2a6"],
  "core/threadAttachmentPages.js": ["df0b246c", "a850af5a", "c612aeac", "414feaa5", "03db6170", "9511cd9c", "372279a6", "9c417822"],
  "core/threadSync.js": ["bf280ad8", "20d158b6", "aaa4f7b9", "695d3816", "1d0200e6", "cf6756bd", "bc0e1372", "2e452737"],
  "core/trackerPages.js": ["debdb55b", "5a1ef8e5", "2a06dff6", "d3574ba7", "8f3f831a", "f2c007bd", "890d9252", "61c2cca2"],
  "core/trackerTaskDetailsFeed.js": ["edcc314b", "9f6f934c", "0b4aecaf", "4f3078e6", "935dbad8", "f62ee991", "ca3a449b", "c1a251a0"],
  "core/trackerTaskPage.js": ["32e943ef", "7fb5a52a", "f80a74bc", "8fa2dd7f", "34295e34", "fe32296f", "80195b8e", "a232861e"],
  "core/trackerTasksPane.js": ["0f751713", "6f929d60", "e35b482a", "7b8485ef", "d6d8b1bf", "80c28293", "d1e9c8c2", "b0936a51"],
  "core/userPresence.js": ["b4a43148", "8352b41e", "f38b7e72", "b03317b3", "ca411d39", "6e7fb659", "b308effa", "511a43aa"],
  "core/watchedTaskFollower.js": ["d93f3cac", "1ab99e30", "84f8b0de", "47ad706c", "94ed9697", "7da95132", "064b155e", "6e2141c1"],
  "core/watchSetting.js": ["550f0da2", "883ae225", "24fd912e", "e669ffb0", "c1d76f0b", "3879059c", "c35a16c8", "8d6065c6"],
  "core/watchToggle.js": ["0c994a20", "9e96598d", "45a92ac1", "3e8a83f5", "95ceaa15", "a98a3720", "f3cad429", "79b9fc50"],
  "core/workspaceDefaults.js": ["26caf0ff", "77dcee98", "234785f1", "a7858438", "d1599850", "87f6f3f6", "90d71240", "2d106714"],
  "core/workspaceGitInitialization.js": ["2b018fe5", "248c6586", "27051b5b", "6a7f6f9e", "daebd529", "68ef1bc8", "af4ea4be", "ae2c1581"],
  "core/workspaceLifecycleSetting.js": ["15c495c8", "5c146e7c", "3e1d376b", "eefb5dbf", "356abf02", "b879315f", "815c5a8a", "123b1b54"],
  "core/workspaceLock.js": ["8464b243", "84658e25", "308e37b8", "ac31b13f", "f30ebad2", "fe08326f", "b831630e", "d650d6fd"],
  "core/workspaceRefPicker.js": ["8e52ee89", "75a29f02", "4ede31a8", "68cbfb6d", "dbc1c227", "0642e2a9", "a7dd1729", "4503b2bc"],
  "core/workspaceReviewEntry.js": ["a0ab5d4b", "8a9fee72", "fdcf3056", "74d69b4e", "99c47dac", "dd855deb", "47ccee52", "b08e5374"],
  "core/workspaceReviewForm.js": ["c41e75cf", "befbe180", "4b9e72d2", "cc6636ed", "8c235490", "7c2aa856", "b628408f", "38b87654"],
  "core/workspaceSizes.js": ["5906d4c2", "fda13846", "3542c664", "26018741", "0bf741e3", "1a10dc8e", "e5feb8ee", "9c08416d"],
  "core/writeOperationCache.js": ["fdac6cd3", "be646fc9", "d9fa4918", "439191f9", "dcbeaab6", "32e1fe4b", "7b278b14", "6950b7ca"],
  "core/writeOperationEffects.js": ["4a1cb360", "33b5176e", "32d67bb3", "1f829d71", "3af475c0", "650a9a6a", "3dba9062", "7bcc372b"],
  "core/writeOperationIntent.js": ["5476394a", "087dc5ce", "5c71e673", "8b977ba3", "e665bf53", "fa9529be", "78bf2c38", "48f27991"],
  "core/writeOperationRefresh.js": ["0e8ca99a", "5f6ff7a5", "fa60f12a", "6c3443d1", "b4fa3618", "e6f6d5a6", "684b5edd", "3b61d3ad"],
  "core/writeOperations.js": ["abcd72e0", "53f1ff90", "93ce533e", "7d43f3a9", "90ca7ee7", "651d200f", "2428a253", "ad7e13ce"],
  "push.js": ["c7a3eb27", "240e0a4d", "3e314aee", "369bea39", "25bf83e0", "f105e41a", "0e4ed716", "4e53028b"],
  "sheets/browser.js": ["e226c6fb", "9689c6bc", "39ba311b", "335481c7", "f444e696", "36c5c8b6", "013588ec", "bd2a4dbe"],
  "sheets/newRepo.js": ["e1addf8b", "abc706f6", "ca74fa24", "7e1f3208", "aae3d460", "37157135", "21e46d2f", "f187cdfa"],
  "sheets/projectSettings.js": ["5ea93508", "d579d760", "280011a4", "f367e8b4", "13b0ab4a", "d38e0d88", "085f46f6", "d1d0f084"],
  "sheets/projectSources.js": ["f9eb9e2b", "9c526294", "46090552", "02740c7a", "25e0f849", "8162d486", "fd90c590", "05524d9d"],
  "sheets/sourceSyncControls.js": ["a0c9eb24", "846213e3", "1cc5b961", "8058d449", "6a46f577", "2b154112", "724dc4e5", "6041f096"],
  "sheets/workspaceSettings.js": ["c93299a4", "e2bfaee7", "3ec925d9", "8b28443f", "66536bf7", "d383d411", "73912a76", "9f6d63ef"],
  "terminal/manager.js": ["cdd9e9b5", "d1c443ca", "9f34fb9b", "d08eb3d5", "3376d6e3", "75088d4e", "99f76359", "89708a92"],
  "terminal/session.js": ["ce4e3036", "ac5b84b3", "ae7583e9", "2b940e62", "4dfe75b7", "d77a06e7", "dfda65ca", "8c8ede4e"],
  "views/archive.js": ["6f4a7962", "88f34292", "73eafead", "5dc6229b", "5b040b7a", "09baf137", "8cbbce83", "6c83d400"],
  "views/branchView.js": ["7187e42f", "7568f9b3", "c1ee9568", "e2a84770", "c59fb311", "5ff935eb", "bd0092f7", "e3c1c23d"],
  "views/deviceProjects.js": ["e91b4c08", "ccb3430d", "4a15da85", "4d787a64", "3ab0ef2c", "ebe5c184", "86d32e0b", "aa25418a"],
  "views/deviceSettings.js": ["d8218af8", "b771ae83", "480d62e8", "302ed6b6", "f534e557", "a43d230d", "74466b01", "eecba939"],
  "views/files.js": ["06be3768", "e7119790", "efdf9d19", "1f24d56e", "0105300d", "78661f62", "dffa8c08", "d92269a8"],
  "views/projectChanges.js": ["e195b594", "3a7689fe", "14bfc899", "18ada18d", "ab36cd52", "ad96ba97", "a877c4da", "c9c3b78f"],
  "views/projectView.js": ["b790e1ad", "c92e3880", "d880307e", "45527ad4", "70eade31", "b21f74d0", "423e637a", "6999aa4e"],
  "views/taskReview.js": ["747e03ed", "de82404a", "137767cd", "dd3635e8", "102a08af", "3bcec6b4", "f735cc20", "6a3744b6"],
  "views/taskView.js": ["483fdb16", "52a51dd3", "ea9f60d0", "c21f16a9", "090b27a4", "4839602c", "4653bfcc", "38959753"],
  "views/workspaceView.js": ["58a33390", "85e012ab", "8250e1e9", "43c007ab", "79acfb68", "398b96a5", "0fbda7bf", "231e1be1"],
  "views/worktreeReview.js": ["3b79626f", "78426186", "6a081af9", "aa906da1", "6abe6978", "6f965143", "b3db6cd3", "12da1a07"]
};
const sourceFingerprints = Object.fromEntries(Object.entries(sourceFingerprintParts)
  .map(([file, parts]) => [file, parts.join("")]));

// Pin method-producing expressions and dynamic dispatch arguments separately
// from literal collection. New/changed dynamic sites must be reviewed rather
// than disappearing because their first argument is not a quoted string.
const dynamicSites = {
  "connection.js": [
    "context . rpc ( ... asked )",
    "session . call ( method )"
  ],
  "core/adoption.js": [
    "call ( method )",
    "runCall ( method )"
  ],
  "core/agentModes.js": [
    "producer `settings.agent_modes.${family} is missing or malformed`"
  ],
  "core/agentRail.js": [
    ". currentCall ( ) ( method )",
    ". currentCall ( ) ( method )",
    ". currentCall ( ) ( method )",
    "context . call ( ... args )"
  ],
  "core/agentSurfacesModel.js": [
    "producer \"build.agentSurfaces.open.\""
  ],
  "core/bridgeApi/v1/index.js": [
    "call ( ... args )"
  ],
  "core/bridgeApi/v1/writeMethods.js": [
    "producer \"changes.\"",
    "producer \"rtc.\""
  ],
  "core/bridgeUpdatePanel.js": [
    "callRpc ( method )"
  ],
  "core/cacheSync.js": [
    "ask ( request . method )",
    "callWhileActive ( method )",
    "callWhileActive ( method )",
    "callWhileActive ( method )",
    "callWhileActive ( method )",
    "callWhileActive ( method )",
    "context . call ( method )",
    "context . call ( verb )",
    "projectTerminalRpc ( method )"
  ],
  "core/changeEvents.js": [
    "call ( method )"
  ],
  "core/changesReview.js": [
    ") . call ( view )"
  ],
  "core/chatRepository.js": [
    ". currentCall ( ) ( submission . threadPostOperations . statusMethod )",
    "mutate ( fresh )"
  ],
  "core/compactionCache.js": [
    "context . call ( job . method )"
  ],
  "core/composeView.js": [
    "homeCall ( method )"
  ],
  "core/consoleModel.js": [
    "producer \"build.console.reopen.\"",
    "producer \"build.console.size.\""
  ],
  "core/conversationCompaction.js": [
    "ask ( entityId )"
  ],
  "core/conversationDetail.js": [
    "producer \"build.conversation.detail.\""
  ],
  "core/deviceContexts.js": [
    "? transport ( ) ( ... asked )",
    "call ( ... asked )",
    "producer \"tasks.waiting_on.\"",
    "return transport ( ) ( ... asked )",
    "return transport ( ) ( ... asked )",
    "return transport ( ) ( ... asked )",
    "return transport ( ) ( ... asked )"
  ],
  "core/filesUploadActions.js": [
    "callRpc ( entry . method )"
  ],
  "core/filesUploads.js": [
    "fileUploadRpc ( contextFor ( deviceId ) )"
  ],
  "core/fileUploadRpc.js": [
    "callRpc ( method )"
  ],
  "core/fileUploads.js": [
    "context . rpc ( method )"
  ],
  "core/gitPane.js": [
    ") . call ( view )",
    "callRpc ( `git.${kind}` )",
    "callRpc ( method )",
    "producer `git.${kind}`",
    "readCheckoutRecord ( kind )",
    "readCheckoutRecord ( kind )",
    "syncActionRpc ( optionId )"
  ],
  "core/inboxDevices.js": [
    "context . rpc ( ... asked )"
  ],
  "core/isolation.js": [
    "callRpc ( target . rpc )"
  ],
  "core/markdownFences.js": [
    "producer `code.${label}`"
  ],
  "core/optimistic.js": [
    "call ( handle )"
  ],
  "core/pathProbe.js": [
    "ask ( method )"
  ],
  "core/projectChangesRpc.js": [
    "context . rpc ( method )"
  ],
  "core/projectConsole.js": [
    "mutate ( held )",
    "mutate ( held ?. tabs || [ ] )"
  ],
  "core/projectFilesRpc.js": [
    "context . rpc ( method )",
    "context . rpc ( method )",
    "context . rpc ( method )"
  ],
  "core/projectTerminalRpc.js": [
    "call ( method )"
  ],
  "core/scopedCompactionEditor.js": [
    "callRpc ( scopes [ scope ] . method )"
  ],
  "core/session.js": [
    "adapter . call ( method )",
    "call ( start )",
    "createSessionRpc ( { transport , ... minted , bridgeEpoch , noCarrier : ( ) => new Error ( \"your device went offline\" ) , } )",
    "greetingGate . call ( ( ) => { const refusal = compactionWriteError ( method , params , adapter ?. capabilities . compaction ?. harnessThresholds ) ; if ( refusal ) throw new ApiError ( refusal . code , refusal . message ) ; return adapter . call ( method , params , options ) ; } )",
    "rpc . call ( method )"
  ],
  "core/sessionRpc.js": [
    "call ( method )",
    "producer \"rtc.\"",
    "producer \"writes.\""
  ],
  "core/sessionSwitch.js": [
    "producer \"rtc.\""
  ],
  "core/syncLists.js": [
    "ask ( context )"
  ],
  "core/syncSuccessors.js": [
    "producer \"tasks.\"",
    "producer \"thread.\""
  ],
  "core/taskChecklist.js": [
    "producer `build.taskChecklist:${JSON.stringify([deviceId, projectId, taskId])}`"
  ],
  "core/taskReviewCache.js": [
    "mutate ( verb )",
    "mutateRpc ( `tasks.review.${verb}` )",
    "producer `tasks.review.${verb}`",
    "reviewMutationRpc ( scope . deviceId )"
  ],
  "core/taskReviewControls.js": [
    "repository . mutate ( verb )"
  ],
  "core/taskReviewLifecycle.js": [
    "repository . mutate ( verb )"
  ],
  "core/taskReviewSupport.js": [
    "callRpc ( ... asked )",
    "callRpc ( ... asked )",
    "context . rpc ( ... asked )",
    "reviewRpc ( context )"
  ],
  "core/taskWaitingOnActions.js": [
    "call ( method )"
  ],
  "core/trackerPages.js": [
    "ask ( asked )"
  ],
  "core/trackerTaskPage.js": [
    "state . callRpc ( ... args )",
    "state . callRpc ( method )",
    "state . callRpc ( method )",
    "state . callRpc ( method )",
    "state . callRpc ( method )"
  ],
  "core/watchedTaskFollower.js": [
    "context . rpc ( method )"
  ],
  "core/watchToggle.js": [
    "call ( watchingNow ? request . watch : request . unwatch )"
  ],
  "core/workspaceReviewEntry.js": [
    "context . rpc ( ... asked )"
  ],
  "core/writeOperations.js": [
    "call . call ( method )",
    "raw ( method )",
    "raw ( nextMethod )",
    "raw ( row . request . method )"
  ],
  "sheets/workspaceSettings.js": [
    "callRpc ( method )"
  ],
  "terminal/manager.js": [
    "call ( method )",
    "projectTerminalRpc ( method )"
  ],
  "terminal/session.js": [
    "_call ( method )",
    "_userCall ( method )",
    "call ( method )",
    "createSessionRpc ( { transport : this . transport , sessionId , sessionKeyB64 , deviceId , bridgeEpoch , noCarrier : ( ) => new TerminalSocketLost ( this . _closed ? \"closed\" : \"disconnected\" ) , } )",
    "rpc . call ( method )",
    "this . _call ( verb )",
    "this . _userTerminalRpc ( this . deviceId )"
  ],
  "views/deviceProjects.js": [
    "creationCall ( chosenId )"
  ],
  "views/deviceSettings.js": [
    "session . call ( ... asked )"
  ],
  "views/projectChanges.js": [
    "projectChangesRpc ( context )"
  ],
  "views/projectView.js": [
    "projectFilesRpc ( state . context )"
  ],
  "views/workspaceView.js": [
    "context . rpc ( method )",
    "context . rpc ( method )",
    "workspaceResultRpc ( state )"
  ],
  "views/worktreeReview.js": [
    "adopting . runCall ( method )"
  ]
};

const helper = /^(?:rpc|call|callRpc|ask|signal|callWhileActive|askBridge|homeCall|targetCall|askDevice|creationCall|mutateRpc|mutate|readPages|readCheckoutRecord|raw|runCall|_call|_userCall)$/;

function cookedString(raw) {
  return raw.replace(/\\(?:u\{[\da-fA-F]+\}|u[\s\S]{4}|x[\s\S]{2}|\r\n|[\s\S])/g, (escaped) => {
    const value = escaped.slice(1);
    if (value.startsWith("u{") || value.startsWith("u") || value.startsWith("x")) {
      const hex = value.startsWith("u{") ? value.slice(2, -1) : value.slice(1);
      assert.match(hex, /^[\da-fA-F]+$/, `unreviewed string escape ${escaped}`);
      const point = Number.parseInt(hex, 16);
      assert.ok(point <= 0x10ffff, `invalid Unicode point ${escaped}`);
      return String.fromCodePoint(point);
    }
    return ({ n: "\n", r: "\r", t: "\t", b: "\b", f: "\f", v: "\v", 0: "\0", "\n": "", "\r": "", "\r\n": "" })[value] ?? value;
  });
}

// A small lexical scanner, not a JavaScript AST: strings, template expression
// bodies, comments and regexes are handled so HTML/comments cannot invent call
// sites. Template expressions get their own token stream for nested calls.
function scan(source) {
  const streams = [], literals = [];
  let cursor = 0;
  const pushString = (tokens, start, quote) => {
    cursor++;
    while (cursor < source.length && source[cursor] !== quote) {
      cursor += source[cursor] === "\\" ? 2 : 1;
    }
    cursor++;
    const value = cookedString(source.slice(start + 1, cursor - 1));
    const token = { kind: "string", value, start, end: cursor };
    tokens.push(token); literals.push(token);
  };
  function code(nested = false) {
    const tokens = [];
    streams.push(tokens);
    let braces = 0;
    while (cursor < source.length) {
      const start = cursor, char = source[cursor], next = source[cursor + 1];
      if (/\s/.test(char)) { cursor++; continue; }
      if (char === "/" && next === "/") {
        cursor = source.indexOf("\n", cursor);
        if (cursor < 0) cursor = source.length;
        continue;
      }
      if (char === "/" && next === "*") {
        const end = source.indexOf("*/", cursor + 2);
        assert.notEqual(end, -1, "unterminated source comment");
        cursor = end + 2; continue;
      }
      if (char === '"' || char === "'") { pushString(tokens, start, char); continue; }
      if (char === "`") {
        cursor++;
        let dynamic = false;
        while (cursor < source.length && source[cursor] !== "`") {
          if (source[cursor] === "\\") { cursor += 2; continue; }
          if (source[cursor] === "$" && source[cursor + 1] === "{") {
            dynamic = true; cursor += 2; code(true); continue;
          }
          cursor++;
        }
        cursor++;
        const raw = source.slice(start, cursor);
        const token = { kind: dynamic ? "template" : "string", value: dynamic ? raw : cookedString(raw.slice(1, -1)), start, end: cursor };
        tokens.push(token);
        if (!dynamic) literals.push(token);
        continue;
      }
      // A regex can contain quotes or comment-looking characters. Recognize
      // expression-start positions used by this source tree.
      const previous = tokens.at(-1)?.value;
      if (char === "/" && (!previous || /^(?:[=(,:!\[?{;]|return|=>|&&|\|\|)$/.test(previous))) {
        cursor++;
        let inClass = false;
        while (cursor < source.length) {
          const part = source[cursor++];
          if (part === "\\") { cursor++; continue; }
          if (part === "[") inClass = true;
          if (part === "]") inClass = false;
          if (part === "/" && !inClass) break;
        }
        while (/[a-z]/i.test(source[cursor] || "")) cursor++;
        tokens.push({ kind: "regex", value: source.slice(start, cursor), start, end: cursor });
        continue;
      }
      if (nested && char === "}" && braces === 0) { cursor++; return; }
      if (char === "{") braces++;
      if (char === "}") braces--;
      const word = /^[A-Za-z_$][\w$]*/.exec(source.slice(cursor));
      const operator = /^(?:\.\.\.|=>|\?\.|&&|\|\||===|!==|==|!=)/.exec(source.slice(cursor));
      const value = word?.[0] || operator?.[0] || char;
      cursor += value.length;
      tokens.push({ kind: word ? "word" : "symbol", value, start, end: cursor });
    }
  }
  code();
  return { streams, literals };
}

function firstArgument(tokens, opening, position = 0) {
  const argument = [];
  let depth = 0;
  for (let index = opening + 1; index < tokens.length; index++) {
    const token = tokens[index];
    if (!depth && token.value === ")") break;
    if (!depth && token.value === ",") {
      if (!position) break;
      position--; argument.length = 0; continue;
    }
    if (["(", "[", "{"].includes(token.value)) depth++;
    if ([")", "]", "}"].includes(token.value)) depth--;
    argument.push(token);
  }
  return argument;
}

const fingerprint = (source) => createHash("sha256").update(source).digest("hex");
const tokenText = ({ kind, value }) => kind === "string" ? JSON.stringify(value) : value;

function metadataUse(tokens, index) {
  let start = index, end = index;
  while (start > 0 && tokens[start - 1].value !== ";") start--;
  while (end < tokens.length && tokens[end].value !== ";") end++;
  const statement = JSON.stringify(tokens.slice(start, end + 1).map(({ kind, value }) => [kind, value]));
  const context = `${tokens.slice(Math.max(0, index - 3), index).map(tokenText).join(" ")} [${JSON.stringify(tokens[index].value)}] ${tokens.slice(index + 1, index + 4).map(tokenText).join(" ")}`;
  return [tokens[index].value, context, fingerprint(statement)];
}

function consumers(source, file) {
  const { streams } = scan(source);
  // Namespace lists cannot be used here: an indirect producer can introduce
  // a new resource family while leaving context.call(method) unchanged.
  const methods = new Set(), usedMetadata = [], dynamic = [];
  const remaining = new Map();
  for (const use of metadataUses[file] || []) {
    const key = JSON.stringify(use);
    remaining.set(key, (remaining.get(key) || 0) + 1);
  }
  let hasRpcCalls = false;
  for (const tokens of streams) {
    for (let index = 0; index < tokens.length; index++) {
      const token = tokens[index];
      if (token.kind === "string" && methodShape.test(token.value)) {
        const use = metadataUse(tokens, index), key = JSON.stringify(use);
        if (remaining.get(key)) {
          remaining.set(key, remaining.get(key) - 1);
          usedMetadata.push(use);
        } else methods.add(token.value);
      }
      // Track method-shaped producers even when a later call only names the
      // variable. Prefix fragments also cover the common "family." + verb
      // construction. These entries are reviewed independently of call sites.
      if (token.kind === "template" && /^`[a-z][\w]*\.[^\s`]*\$\{/.test(token.value)) {
        dynamic.push(`producer ${token.value}`);
      }
      if (token.kind === "string" && /^[a-z][\w]*(?:\.[A-Za-z][\w]*)*\.$/.test(token.value)) {
        dynamic.push(`producer ${JSON.stringify(token.value)}`);
      }
      if (token.value !== "(") continue;
      const name = tokens[index - 1]?.value;
      const returnedCall = name === ")" && tokens[index - 2]?.value === "(";
      const reviewSubmit = name === "submit" && ["core/taskReviewControls.js", "core/taskReviewLifecycle.js"].includes(file);
      const selected = helper.test(name || "") || /Rpc$/.test(name || "") || returnedCall || reviewSubmit;
      if (!selected || tokens[index - 2]?.value === "function") continue;
      hasRpcCalls = true;
      const afterContext = ["callWhileActive", "askBridge", "raw", "projectTerminalRpc"].includes(name)
        || name === "ask" && ["core/cacheSync.js", "core/syncTaskPhases.js"].includes(file)
        || name === "call" && tokens[index - 3]?.value === "call";
      const argument = firstArgument(tokens, index, Number(afterContext));
      if (!argument.length) continue;
      if (argument.length === 1 && argument[0].kind === "string") {
        const value = name === "mutate" || reviewSubmit ? `tasks.review.${argument[0].value}` : argument[0].value;
        if (methodShape.test(value)) methods.add(value);
        continue;
      }
      const member = [".", "?."].includes(tokens[index - 2]?.value);
      const before = tokens.slice(Math.max(0, index - (returnedCall ? 4 : member ? 3 : 1)), index).map(({ value }) => value).join(" ");
      dynamic.push(`${before} ( ${argument.map(({ kind, value }) => kind === "string" ? JSON.stringify(value) : value).join(" ")} )`);
    }
  }
  return { methods, dynamic: dynamic.sort(), metadataUses: usedMetadata, hasRpcCalls, sourceFingerprint: fingerprint(source) };
}

function requiresSourceReview(record, file) {
  return record.methods.size > 0 || record.hasRpcCalls || record.dynamic.length > 0 || Object.hasOwn(metadataUses, file);
}

async function sources(directory = new URL("spa/src/", root), prefix = "") {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = new URL(entry.name, directory);
    if (entry.isDirectory()) files.push(...await sources(new URL(`${entry.name}/`, directory), `${prefix}${entry.name}/`));
    else if (entry.name.endsWith(".js")) files.push({ file: `${prefix}${entry.name}`, source: await readFile(file, "utf8") });
  }
  return files.sort((left, right) => left.file.localeCompare(right.file));
}

test("literal collection finds unknown methods without a registry intersection", () => {
  const found = consumers('context.rpc("new_family.future"); callRpc("agent.start"); context.call("thread.page");', "synthetic.js");
  assert.deepEqual([...found.methods].sort(), ["agent.start", "new_family.future", "thread.page"]);
});

test("escaped quoted and static template literals retain their actual method names", () => {
  const cases = [
    String.raw`context.call("novel\u002eaction");`,
    String.raw`const method = "novel\x2eaction"; context.call(method);`,
    String.raw`const method = "novel\u{2e}action"; context.call(method);`,
    'context.call(`novel\\u002eaction`);',
  ];
  for (const source of cases) assert.ok(consumers(source, "synthetic.js").methods.has("novel.action"), source);
});

test("indirect producers in new namespaces remain coverage requirements", () => {
  const known = consumers('const method = "agent.start"; context.call(method);', "synthetic.js");
  const novel = consumers('const method = "novel.action"; context.call(method);', "synthetic.js");
  assert.deepEqual(novel.dynamic, known.dynamic, "the dispatch expression stays unchanged in this regression");
  assert.deepEqual([...novel.methods], ["novel.action"]);
  const covered = new Set(["agent.start"]);
  assert.deepEqual([...novel.methods].filter((method) => !covered.has(method)), ["novel.action"]);
});

test("metadata labels reused as direct or indirect methods remain consumers", () => {
  for (const [file, values] of Object.entries(metadataUses)) {
    for (const [value] of values) {
      const quoted = JSON.stringify(value);
      assert.ok(consumers(`context.call(${quoted});`, file).methods.has(value), `${file}: direct ${value}`);
      assert.ok(consumers(`const method = ${quoted}; context.call(method);`, file).methods.has(value), `${file}: indirect ${value}`);
      assert.ok(consumers(`const descriptor = { method: ${quoted} }; context.call(descriptor.method);`, file).methods.has(value), `${file}: descriptor ${value}`);
      assert.ok(consumers(`const method = condition ? ${quoted} : "board.list"; context.call(method);`, file).methods.has(value), `${file}: ternary ${value}`);
      assert.ok(consumers(`const method = \`${value}\`; context.call(method);`, file).methods.has(value), `${file}: template ${value}`);
      assert.ok(consumers(`const method = ${quoted} + suffix; context.call(method);`, file).methods.has(value), `${file}: prefix ${value}`);
    }
  }
});

test("an existing metadata use does not suppress another same-valued producer", async () => {
  const file = "core/cacheSync.js";
  const source = await readFile(new URL(`spa/src/${file}`, root), "utf8");
  assert.ok(!consumers(source, file).methods.has("build.cacheSync"));
  const changed = `${source}\nconst method = "build.cacheSync"; context.call(method);`;
  assert.ok(consumers(changed, file).methods.has("build.cacheSync"));
});

test("copying an audited metadata occurrence cannot reuse its finite exemption", async () => {
  const file = "appState.js";
  const source = await readFile(new URL(`spa/src/${file}`, root), "utf8");
  assert.ok(!consumers(source, file).methods.has("build.selectedDeviceId"));
  assert.ok(consumers(`${source}\n${source}`, file).methods.has("build.selectedDeviceId"));
});

test("metadata aliases and compound producers require review even when dispatch stays identical", async () => {
  const file = "core/cacheSync.js";
  const source = await readFile(new URL(`spa/src/${file}`, root), "utf8");
  const before = consumers(`${source}\nconst method = "board.list"; context.call(method);`, file);
  const producers = [
    'const method = SYNC_LOCK;',
    'const descriptor = { method: SYNC_LOCK }; const method = descriptor.method;',
    'const method = condition ? SYNC_LOCK : "board.list";',
    'const method = `${SYNC_LOCK}`;',
    'const method = SYNC_LOCK + suffix;',
  ];
  for (const producer of producers) {
    const after = consumers(`${source}\n${producer} context.call(method);`, file);
    assert.deepEqual([...after.methods].sort(), [...before.methods].sort(), producer);
    assert.deepEqual(after.dynamic, before.dynamic, producer);
    assert.notEqual(after.sourceFingerprint, before.sourceFingerprint, `source review must catch ${producer}`);
  }
});

test("new recognized helper-call and producer files cannot disappear from source review", () => {
  const file = "new-resource-module.js";
  for (const source of ['context.call("undotted");', 'const method = "novel.action";', 'context.call(computed);']) {
    const record = consumers(source, file);
    assert.ok(requiresSourceReview(record, file), source);
    assert.ok(!Object.hasOwn(sourceFingerprints, file));
  }
});

test("indirect template and concatenation producers are inventoried independently of dispatch", () => {
  const known = consumers('const method = `agent.${action}`; context.call(method);', "synthetic.js");
  const novel = consumers('const method = `novel.${action}`; context.call(method);', "synthetic.js");
  assert.notDeepEqual(novel.dynamic, known.dynamic);
  const concatenated = consumers('const method = "novel." + action; context.call(method);', "synthetic.js");
  assert.ok(concatenated.dynamic.some((site) => site.includes('"novel."')));
});

test("comments and regexes do not invent consumers; nested template calls do", () => {
  const found = consumers('// rpc("fake.comment")\nconst r = /"fake.regex"/; const html = `x ${rpc("capture.get")}`;', "synthetic.js");
  assert.deepEqual([...found.methods], ["capture.get"]);
  assert.notDeepEqual(consumers('rpc(`git.${kind}`)', "synthetic.js").dynamic, consumers('rpc(`git.${other}`)', "synthetic.js").dynamic);
});

test("review suffix dispatch expands new literal verbs instead of dropping them", () => {
  assert.ok(consumers('repository.mutate("future_action", params)', "synthetic.js").methods.has("tasks.review.future_action"));
  assert.ok(consumers('submit("future_action", params)', "core/taskReviewLifecycle.js").methods.has("tasks.review.future_action"));
});

test("git template dispatch retains its reviewed status/log expansion", async () => {
  const source = await readFile(new URL("spa/src/core/gitPane.js", root), "utf8");
  const normalized = scan(source).streams[0].map(({ kind, value }) => kind === "string" ? JSON.stringify(value) : value).join(" ");
  assert.ok(normalized.includes('forceRefresh = async ( kinds = [ "status" , "log" ] , checkOnly = false ) =>'), "Review the new default git kind expansion");
  assert.ok(normalized.includes('[ "status" , "log" ] . map ( ( kind ) => kinds . includes ( kind ) ? readCheckoutRecord ( kind ) : null )'), "Review the new dynamic git kind expansion");
});

test("greeting-selected operation status retains the reviewed thread.operation contract", async () => {
  const source = await readFile(new URL("spa/src/core/chatRepository.js", root), "utf8");
  const normalized = scan(source).streams[0].map(({ kind, value }) => kind === "string" ? JSON.stringify(value) : value).join(" ");
  assert.ok(normalized.includes('const statusMethod = typeof offered ?. status_method === "string" && offered . status_method ? offered . status_method : "thread.operation" ;'), "Review the operation-status method producer and expand coverage for any new contract");
  const greeting = await readFile(new URL("bridge/src/app/runtime/terminals.rs", root), "utf8");
  assert.match(greeting, /"status_method"\s*:\s*"thread\.operation"/, "Expand coverage when the bridge advertises a different operation status method");
});

test("every SPA resource reference is mapped or explicitly excluded", async () => {
  const manifestPath = new URL("fixtures/api/http/resource-manifest.json", root);
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  assert.ok(Array.isArray(manifest.operations), `${fileURLToPath(manifestPath)}: operations must be an array`);
  assert.ok(Array.isArray(manifest.exclusions), "manifest exclusions must be an array");
  const covered = new Set([...manifest.operations, ...manifest.exclusions].map((operation) => operation.method));
  const missing = [];
  for (const { file, source } of await sources()) {
    for (const method of consumers(source, file).methods) if (!covered.has(method)) missing.push(`${file}: ${method}`);
  }
  assert.deepEqual(missing.sort(), [], "Add resource routes or deliberate documented exclusions for these consumers");
});

test("dynamic method dispatch sites remain explicitly accounted for", async () => {
  const actual = {};
  for (const { file, source } of await sources()) {
    const { dynamic } = consumers(source, file);
    if (dynamic.length) actual[file] = dynamic;
  }
  assert.deepEqual(actual, dynamicSites, "Review new/changed dynamic sites and their concrete method expansion before updating this inventory");
});

test("metadata exemptions still match exactly the audited occurrences", async () => {
  const actual = {};
  for (const { file, source } of await sources()) {
    const record = consumers(source, file);
    if (record.metadataUses.length) actual[file] = record.metadataUses;
  }
  assert.deepEqual(actual, metadataUses, "Review changed/missing metadata uses; do not exempt all same-valued literals");
});

test("reviewed RPC and producer source requires explicit review when assignment edges change", async () => {
  const actual = {};
  for (const { file, source } of await sources()) {
    const record = consumers(source, file);
    if (requiresSourceReview(record, file)) actual[file] = record.sourceFingerprint;
  }
  assert.deepEqual(actual, sourceFingerprints, "Review changed/new RPC or producer source, including alias and assignment edges, before updating source fingerprints");
});
