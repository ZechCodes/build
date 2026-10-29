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
const sourceFingerprints = {
  "app.js": "529fcd475365bb975a6dcfc230089f9e831863fcb89bde77bf53ffd31b761c41",
  "appState.js": "d24d9edc6e4988f08a5289ee6c449b3f91bdbe427cd43ada7a9df6a4acd39971",
  "connection.js": "314548c168c905142ee3e9b5c53f979822aba77b060e5c13e5cc23f0f1662147",
  "core/activityRuns.js": "2bd80d81ad2ddde9da2fc19495b7b7ba4d0a86f429a52906009b6fc55221d710",
  "core/adoption.js": "350b90d59e535f4b15adf2ef7eacd8c0fb946020b6130cb8ce729fedbb90597e",
  "core/agentDefaults.js": "f856d23d2c446c14c6e4d78ee11e737d75e1fbc1fa79564d2ef20a2b1cdbbcb0",
  "core/agentModes.js": "5764f59d200944fde1fb485fa1b58c05633b7f0ff9b7cbcd4e62b9db31bcb3a8",
  "core/agentRail.js": "5a8293a8160dddf5ff4ae6e8c7c4dee863f484b6c8a205ca59b70a141aa11a6a",
  "core/agentRailContext.js": "e175542d376171c6e04b61df847af2cfa3a2c56e8fbeb9139593b4cb50809f5c",
  "core/agentRolesPanel.js": "c05b7c965849ec7f0d258bc296a7795e2a4663f457e78316de145b90699b4a91",
  "core/agentSurfacesModel.js": "546cf5aa3a478c634a0f02f54eafcae553a5ffd33ad4f563c075aa93dcbbef9d",
  "core/bridgeApi/v1/compactionParams.js": "506db6e2f8148bc4925e889b61c6fe15d8cf74e2ef7e468fb610115f7132ce5b",
  "core/bridgeApi/v1/index.js": "7124f4e6ac45c6fdd5ffd9c795967cc287d43e61e2bef7b06746b8c1e5720582",
  "core/bridgeApi/v1/writeMethods.js": "f311847fda193503678520ece289e013fbbcc9f926f2aae932c184fe7f27b03d",
  "core/bridgeEpoch.js": "e04a0a116535b020abcac20c972a554f8d51fbb96ef07fd816062ed09cd4d213",
  "core/bridgeUpdatePanel.js": "f9b19cc74abdfc5ad159194d49ee91edd39ab2e715a6a80f0cb51530623ca8e5",
  "core/bridgeUpdates.js": "279cb9320495aa46c347f93b10a492d58d29b0a9e9309a9a91de7993e5f40ad3",
  "core/cacheSync.js": "a8f7ffd725d0a21d5160f9e10166e608bd45be2259951a3c685429df2f71c39d",
  "core/captureDecisionView.js": "f9f42a7dd94162aa5aea82e585e6bcb273ffce612ac992148b78b55f30a7a61a",
  "core/changeEvents.js": "2de5688656ad09a99b2c303adfb97b7c6b7d67b6932183bbaddc42c1313ed272",
  "core/changesComments.js": "f258a3f15b42f974ba0af04ff955c0ff1cdd17d3353ce276c7dd49b6eb1c06be",
  "core/changesModel.js": "1a2a148524c755c4706c0111981a47652cc7236e4ff87fce4a7640ba13a75672",
  "core/changesReview.js": "efa3d8fd2f936aea3c017fef5e7e463e5b6485cfabc5c8096e9a263642ac75ec",
  "core/chatChoiceController.js": "d257b6c5f32d9067226af14cbd8ec71e718bf1881e994ef166edcceb4499fa61",
  "core/chatRepository.js": "a7331089a8f626b33c5700e5e726dfbe7aeb4d7559d712436bb28ca26e0dd982",
  "core/compactionCache.js": "041bfd3e820024deec19e3f6c7ce04ad4adecae65f78a182d12f13cb89e82556",
  "core/compose.js": "012665e787915aea9314f63c7b03643870ec846e09d98f92e8746b91051b004e",
  "core/composeView.js": "b560ac0027513049482b025b79f330a544f3a97ff2c88efa2f36708ba8951245",
  "core/consoleModel.js": "4feba529663ba4716fe6e2a13eb66e5c815545f8f70a5b48a6248d7594d9ed20",
  "core/conversationCompaction.js": "4e0764452fd7fb1d1ca9681eb39ea81401d5b9f926f91118a52788ca54220e9e",
  "core/conversationDetail.js": "a54ac9f8f54c61d4cea363a5b098964f16ecc016490b853d76d94602ad5a773a",
  "core/createWork.js": "42444ebc8ae4117eba2cdde67d740ba31850d7a6a63c9fd25078496418d57196",
  "core/defaultHarness.js": "b7ed5ad69fdfdc3584dd803d3c0e95fd729040164cb2993897723e942ace812d",
  "core/deviceContexts.js": "36c494bd9e3a9ada5feb60e8296573df9f3f36dc252afebd97642d91d82ec699",
  "core/diffSort.js": "9ef743b816f0a5ae1d9ec36a455f0e76c04a09028deca6c770acd150dbe96ac6",
  "core/directoryRail.js": "98080c38f21522853e02586810652120d12c8eadcfa571c66c3476c8b6b56ef2",
  "core/fileDiffs.js": "94424fd1118dcd8d6c511bb9642d4822e4ebd5c2ffd9b04141d478ef20b2347d",
  "core/filesUploadActions.js": "a3bbb07874d426930534c27d8bf50ca41b0cccc7b6cc2162feb1f0ef53188d20",
  "core/filesUploads.js": "3270237979d5ef63c486dc6790cbc884c0cbf79d28deabc3f4202a02d95a8bd3",
  "core/fileUploadRpc.js": "7891a6ca183a9b67b7fe41fcba71c70a1896ec752809155c1375617fa9ab96d0",
  "core/fileUploads.js": "a1a4c2df5ef03503483e7b99099e04a077c9e865a22a23dda704cd796678f239",
  "core/fileUploadsModel.js": "d11fdd798bb9318cfbc2cff229b169309d73b3ee89d5d9767e2b756a69d40aac",
  "core/githubRepos.js": "4526fb16bfe42e0f57b963eafe6238730fe9ccce521ecc83b9117e59af64462f",
  "core/gitPane.js": "7a5a41a06e6035564ba87c41f7a231e865fe67f31094f3635aacb8394e647b57",
  "core/inboxCaptures.js": "48a7f5fbb2132a835901fb314fb0c5ae970052972d0b9db6be2c3e6d1271b663",
  "core/inboxDevices.js": "f9a293e4dff4b2b4f5fc7940f5c1f48da66a5718238826a6e3dc6b9f4060a79e",
  "core/inboxSeen.js": "8131bfc18d81301fe91c3d8044cb2dec5c60d797eb8516a43b49d94aca32676a",
  "core/inboxView.js": "433300bb6f8e561b66b4fe10c55cbfe5670a7736ab4df347d6f4bba43ae07401",
  "core/isolation.js": "1b646651086eea7fb0a97d6f8993ed777e6e246e4110fbc27defb07677f79221",
  "core/markdownFences.js": "1005739668214f4dd193015e2552698ec48313df6df2e2237e3bc8372805a4bb",
  "core/modelCatalog.js": "c293ea11fff0969f7852c8cd0c252f82fe6661599591941725a661ecc93fd9b4",
  "core/navigationVisits.js": "f8d32f7a6bf64af68186647502ebd8fc3a0be61cc9f0d9f3e1cc0c06dd4dc05a",
  "core/optimistic.js": "664afc7e9083408c39d1e0855a5314849d315ad7dcbe2655b119e5086f3ef787",
  "core/pathProbe.js": "1518fd69692eec10f8018014c81c6ee14edda84eefee05d6472437de73c96fea",
  "core/peerLink.js": "6c6acb2728aa36e60258ba0f303f308a45b3ffb6701efb20ae563ff76a4ddb2d",
  "core/projectAgentsCache.js": "692ce6e629188041a638ab36772487e7397715fedba4c0f54e938ee86bbdb7ff",
  "core/projectAgentSetting.js": "cf7d410af3bc1eb41b4e78339dc4cf5023b0128001636e2772e8edb1e4a7e379",
  "core/projectChangesRpc.js": "613fe38159f4a3a408a518ed4e53526121ca4a585e5011d9c0acd52ebb8e5014",
  "core/projectConsole.js": "b44678cc4c2dbc2a188695a2307b48834d4fb733091622af3c6480669d1e32bb",
  "core/projectFilesRpc.js": "e84ae7b7ceffb0cc7ff4c6cdcfc0c05f13a0d831079f3b3e5dae2e56ce4686ac",
  "core/projectTerminalRpc.js": "b391c6dcaa385e26aadfb8b8f6ef5cc99a8e7a79d9d757bbcdd1356c867cadea",
  "core/pushKeySync.js": "ecd9373a3f2dd8cbaf448d181098ca18ef83c7bb923a9c90b5d5f0429ac75438",
  "core/railMode.js": "9a3738d3e4361bdffef94f86f803203fefbe6cbf8513e3f75fc7602cdd4e5e6b",
  "core/readStateRepair.js": "cf9c2d3b411a9904c9edb8e933ae46f6d879a630a802c545b665fb8c44cb7422",
  "core/revisionBodies.js": "60606411ec3a6f2c7c923581f22c39cd43dd8a0469ff5c40e4e490cbe3119193",
  "core/scopedCompactionEditor.js": "b5d366f590a2be6ab5c01379ccad51987298a4cfbdcbdc882e022cd4847d6330",
  "core/session.js": "3d3cf7dd79e4e284e35b0919e70c73b084b3ec43325e1ae6fc5b5b9dab43100a",
  "core/sessionRpc.js": "b4643d7009686b0664a93192a5d2f604c01907cba777026a7364e2d5c53f74a9",
  "core/sessionSwitch.js": "dc232a715b006195d7910b8e8c59e611a36dd2648bbcfcc80d8739f3371accc7",
  "core/shell.js": "c9e2a17e6ae389f1e7a60035dfc83868dc85c17d564daa9f9b3e097b391bcd33",
  "core/sqlReadCredits.js": "bd48f63c6ba332ba579d3bb17246f83f80932ec8e656744312da1fae2144302e",
  "core/syncDiagnostics.js": "1060f9bab9d583f793db302a9ecabc6808ff098a2119a4b8fdc89f9a74403b01",
  "core/syncLists.js": "e44aee99346eb0271e6486f1dac9a7ac9acdd7b81c4a00f4ce196d01e22f5292",
  "core/syncSuccessors.js": "8f215712252114ceb5f282bf3b818ded2bfe5a5260ced14f965e228e9af31a83",
  "core/syncTaskPhases.js": "8ded146de8dd689916ffd0a44b38ca1fab2b34002c745f68c2096ffdc4845622",
  "core/taskAttachmentBodies.js": "7acfa90a4413c7df89207db807fa4c16b21cc95fdc3bf412eb0c14de671d3006",
  "core/taskChecklist.js": "74b432600b2d0ae06ea4ea00c53db5e48830cc22090c1067a4ffbd6f8ed3cae6",
  "core/taskComposer.js": "44233e6ad60f8251f724f7bcaf16b3192c205f315a10e50bb233a25fed90f8ad",
  "core/taskReviewActions.js": "9e0e7f8156f56199855d3846a7c38661bcc2d05233bc24692299c4a56c0bb485",
  "core/taskReviewCache.js": "baa998911615149dfa8810f035d28a10be31a52041c6c1543bf6b31d44e4a700",
  "core/taskReviewChanges.js": "5bda4fe876062ca2b8b03a1154479aab60eb34c004dfbdc73bdbee0d454737ed",
  "core/taskReviewControls.js": "b694c9e419686ec0d0c8e7ef5ce2383457a0c5afbce0145c5c889031fb4f998c",
  "core/taskReviewFeedback.js": "c66d3512596e587897c8c00cede58c716893718a79654d4546c47189cb5124ab",
  "core/taskReviewFiles.js": "f718c0f40ddfa4b1bdff017a25f15f7ba50ef953dc2123d5d3acd7ae2f11fb2f",
  "core/taskReviewLifecycle.js": "04b749fb1781936b6c4264abb48cb304339a1a4e28617a6fcc24e0bf0f8ee6d3",
  "core/taskReviewMerge.js": "805e7cd6c7f927019b8b5f086c8e985da1de4e1ba60280e4aa4249346b83af9e",
  "core/taskReviewReclaim.js": "39f9773b9fb74944f19e9d448698c30dcbd907a0bde50825b8601ff9a2100184",
  "core/taskReviewSupport.js": "31e905424726fa94eaf3677fb9d8a43743a69e5a0c9de2c3967250ca3cbd5571",
  "core/taskView.js": "547b447a1b0897028da986a097cbffed86cd8be0dfabf68023671d546ac7ef11",
  "core/taskWaitingOnActions.js": "ba117c3fa39570246f0346c2a4e5a3002b6533860e1ff268216a0f349d20d4e0",
  "core/taskWaitingOnPage.js": "b5a78897c7cfc8dc8a1ad2e0c77cad0a874280e01b39469737da25477a1c3636",
  "core/theme.js": "5f821f5d933be0fac04dd60c8ed91a2a4fd56dbbab2c8e04e86ee7f560d8e2a6",
  "core/threadAttachmentPages.js": "df0b246ca850af5ac612aeac414feaa503db61709511cd9c372279a69c417822",
  "core/threadSync.js": "bf280ad820d158b6aaa4f7b9695d38161d0200e6cf6756bdbc0e13722e452737",
  "core/trackerPages.js": "debdb55b5a1ef8e52a06dff6d3574ba78f3f831af2c007bd890d925261c2cca2",
  "core/trackerTaskDetailsFeed.js": "edcc314b9f6f934c0b4aecaf4f3078e6935dbad8f62ee991ca3a449bc1a251a0",
  "core/trackerTaskPage.js": "32e943ef7fb5a52af80a74bc8fa2dd7f34295e34fe32296f80195b8ea232861e",
  "core/trackerTasksPane.js": "0f7517136f929d60e35b482a7b8485efd6d8b1bf80c28293d1e9c8c2b0936a51",
  "core/userPresence.js": "b4a431488352b41ef38b7e72b03317b3ca411d396e7fb659b308effa511a43aa",
  "core/watchedTaskFollower.js": "d93f3cac1ab99e3084f8b0de47ad706c94ed96977da95132064b155e6e2141c1",
  "core/watchSetting.js": "550f0da2883ae22524fd912ee669ffb0c1d76f0b3879059cc35a16c88d6065c6",
  "core/watchToggle.js": "0c994a209e96598d45a92ac13e8a83f595ceaa15a98a3720f3cad42979b9fc50",
  "core/workspaceDefaults.js": "26caf0ff77dcee98234785f1a7858438d159985087f6f3f690d712402d106714",
  "core/workspaceGitInitialization.js": "2b018fe5248c658627051b5b6a7f6f9edaebd52968ef1bc8af4ea4beae2c1581",
  "core/workspaceLifecycleSetting.js": "15c495c85c146e7c3e1d376beefb5dbf356abf02b879315f815c5a8a123b1b54",
  "core/workspaceLock.js": "8464b24384658e25308e37b8ac31b13ff30ebad2fe08326fb831630ed650d6fd",
  "core/workspaceRefPicker.js": "8e52ee8975a29f024ede31a868cbfb6ddbc1c2270642e2a9a7dd17294503b2bc",
  "core/workspaceReviewEntry.js": "a0ab5d4b8a9fee72fdcf305674d69b4e99c47dacdd855deb47ccee52b08e5374",
  "core/workspaceReviewForm.js": "c41e75cfbefbe1804b9e72d2cc6636ed8c2354907c2aa856b628408f38b87654",
  "core/workspaceSizes.js": "5906d4c2fda138463542c664260187410bf741e31a10dc8ee5feb8ee9c08416d",
  "core/writeOperationCache.js": "fdac6cd3be646fc9d9fa4918439191f9dcbeaab632e1fe4b7b278b146950b7ca",
  "core/writeOperationEffects.js": "4a1cb36033b5176e32d67bb31f829d713af475c0650a9a6a3dba90627bcc372b",
  "core/writeOperationIntent.js": "5476394a087dc5ce5c71e6738b977ba3e665bf53fa9529be78bf2c3848f27991",
  "core/writeOperationRefresh.js": "0e8ca99a5f6ff7a5fa60f12a6c3443d1b4fa3618e6f6d5a6684b5edd3b61d3ad",
  "core/writeOperations.js": "abcd72e053f1ff9093ce533e7d43f3a990ca7ee7651d200f2428a253ad7e13ce",
  "push.js": "c7a3eb27240e0a4d3e314aee369bea3925bf83e0f105e41a0e4ed7164e53028b",
  "sheets/browser.js": "e226c6fb9689c6bc39ba311b335481c7f444e69636c5c8b6013588ecbd2a4dbe",
  "sheets/newRepo.js": "e1addf8babc706f6ca74fa247e1f3208aae3d4603715713521e46d2ff187cdfa",
  "sheets/projectSettings.js": "5ea93508d579d760280011a4f367e8b413b0ab4ad38e0d88085f46f6d1d0f084",
  "sheets/projectSources.js": "f9eb9e2b9c5262944609055202740c7a25e0f8498162d486fd90c59005524d9d",
  "sheets/sourceSyncControls.js": "a0c9eb24846213e31cc5b9618058d4496a46f5772b154112724dc4e56041f096",
  "sheets/workspaceSettings.js": "c93299a4e2bfaee73ec925d98b28443f66536bf7d383d41173912a769f6d63ef",
  "terminal/manager.js": "cdd9e9b5d1c443ca9f34fb9bd08eb3d53376d6e375088d4e99f7635989708a92",
  "terminal/session.js": "ce4e3036ac5b84b3ae7583e92b940e624dfe75b7d77a06e7dfda65ca8c8ede4e",
  "views/archive.js": "6f4a796288f3429273eafead5dc6229b5b040b7a09baf1378cbbce836c83d400",
  "views/branchView.js": "7187e42f7568f9b3c1ee9568e2a84770c59fb3115ff935ebbd0092f7e3c1c23d",
  "views/deviceProjects.js": "e91b4c08ccb3430d4a15da854d787a643ab0ef2cebe5c18486d32e0baa25418a",
  "views/deviceSettings.js": "d8218af8b771ae83480d62e8302ed6b6f534e557a43d230d74466b01eecba939",
  "views/files.js": "06be3768e7119790efdf9d191f24d56e0105300d78661f62dffa8c08d92269a8",
  "views/projectChanges.js": "e195b5943a7689fe14bfc89918ada18dab36cd52ad96ba97a877c4dac9c3b78f",
  "views/projectView.js": "b790e1adc92e3880d880307e45527ad470eade31b21f74d0423e637a6999aa4e",
  "views/taskReview.js": "747e03edde82404a137767cddd3635e8102a08af3bcec6b4f735cc206a3744b6",
  "views/taskView.js": "483fdb1652a51dd3ea9f60d0c21f16a9090b27a44839602c4653bfcc38959753",
  "views/workspaceView.js": "58a3339085e012ab8250e1e943c007ab79acfb68398b96a50fbda7bf231e1be1",
  "views/worktreeReview.js": "3b79626f784261866a081af9aa906da16abe69786f965143b3db6cd312da1a07"
};

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
