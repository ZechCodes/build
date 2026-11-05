// Run from any directory: node --test scripts/http-resource-coverage.test.mjs
// This scans consumers independently of the Rust registry: intersecting the
// two would silently discard legacy calls and newly introduced methods.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
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
    ["term.output","[ \"changes\" , [\"term.output\"] , \"term.reset\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["term.reset",", \"term.output\" , [\"term.reset\"] , \"term.closed\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["term.closed",", \"term.reset\" , [\"term.closed\"] , \"rtc.ice\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["rtc.diagnostics",", \"rtc.ice\" , [\"rtc.diagnostics\"] , \"bridge.update_status\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["models.changed",", \"bridge.update_status\" , [\"models.changed\"] , \"harnesses.changed\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["harnesses.changed",", \"models.changed\" , [\"harnesses.changed\"] , \"compaction.changed\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["compaction.changed",", \"harnesses.changed\" , [\"compaction.changed\"] , \"secrets.changed\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["secrets.changed",", \"compaction.changed\" , [\"secrets.changed\"] , \"writes.confirmed\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["writes.confirmed",", \"secrets.changed\" , [\"writes.confirmed\"] , \"bridge.restarting\" ,","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["bridge.restarting",", \"writes.confirmed\" , [\"bridge.restarting\"] , ] )","3e19187efe49908cdfeb0d84c47dc2475bc1dd8e34ff78ddf5033f3f43de41a1"],
    ["secrets.attachments",". has ( [\"secrets.attachments\"] ) , sealedUpload","d602aee9fb486c0ef7a43dd7fc204224b250ee6b9c56e419bed26ac4f56d6adf"],
    ["secrets.sealedUpload",". has ( [\"secrets.sealedUpload\"] ) , metadataEvents","d602aee9fb486c0ef7a43dd7fc204224b250ee6b9c56e419bed26ac4f56d6adf"],
    ["secrets.metadataEvents",". has ( [\"secrets.metadataEvents\"] ) , }","d602aee9fb486c0ef7a43dd7fc204224b250ee6b9c56e419bed26ac4f56d6adf"],
    ["changes.subscriptions",". has ( [\"changes.subscriptions\"] ) ; const","20fb6a0c88c128a1254da272bc70614f9aba7dc095f927a02e0b67717a777c23"],
    ["bridge.epoch",". has ( [\"bridge.epoch\"] ) , ackConfirmMethods","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["bridge.epoch",". has ( [\"bridge.epoch\"] ) ) }","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["changes.filesHints",". has ( [\"changes.filesHints\"] ) , kinds","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["changes.projectSources",". has ( [\"changes.projectSources\"] ) , }","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["git.projectSources",". has ( [\"git.projectSources\"] ) , showPaths","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["git.showPaths",". has ( [\"git.showPaths\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["term.projectSources",". has ( [\"term.projectSources\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["fs.treePaged",". has ( [\"fs.treePaged\"] ) , projectSources","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["fs.projectSources",". has ( [\"fs.projectSources\"] ) , projectSourcePath","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["fs.projectSourcePath",". has ( [\"fs.projectSourcePath\"] ) , uploads","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["fs.uploadBegin",". has ( [\"fs.uploadBegin\"] ) , createDirectory","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["fs.createDirectory",". has ( [\"fs.createDirectory\"] ) , createFile","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["fs.createFile",". has ( [\"fs.createFile\"] ) , }","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["requests.priority",". has ( [\"requests.priority\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["rtc.clientLanCache",". has ( [\"rtc.clientLanCache\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["errors.codes",". has ( [\"errors.codes\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["diffs.perFile",". has ( [\"diffs.perFile\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["bodies.pages",". has ( [\"bodies.pages\"] ) , mediaRawPages","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["fs.mediaRawPages",". has ( [\"fs.mediaRawPages\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["sync.device",". has ( [\"sync.device\"] ) , projectTasks","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["sync.projectTasks",". has ( [\"sync.projectTasks\"] ) , task","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["sync.task",". has ( [\"sync.task\"] ) , }","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.snapshotRetired",". has ( [\"tasks.review.snapshotRetired\"] ) , ...","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.reject",". has ( [\"tasks.review.reject\"] ) , author","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.author",". has ( [\"tasks.review.author\"] ) , verdictDelivery","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.verdictDelivery",". has ( [\"tasks.review.verdictDelivery\"] ) , comments","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.reviewComments",". has ( [\"tasks.reviewComments\"] ) , commits","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.commits",". has ( [\"tasks.review.commits\"] ) , commit","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.commit",". has ( [\"tasks.review.commit\"] ) , since","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.since",". has ( [\"tasks.review.since\"] ) , anchorCommit","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.anchorCommit",". has ( [\"tasks.review.anchorCommit\"] ) , forTask","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.forTask",". has ( [\"tasks.review.forTask\"] ) , taskChoices","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listSummaries",". has ( [\"tasks.listSummaries\"] ) , linkedTaskChoices","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listLinkedWorkspace",". has ( [\"tasks.listLinkedWorkspace\"] ) , pullRequests","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.review.pullRequests",". has ( [\"tasks.review.pullRequests\"] ) , }","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.timelinePaged",". has ( [\"tasks.timelinePaged\"] ) , context","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.context",". has ( [\"tasks.context\"] ) , assignProjectAgent","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.assignProjectAgent",". has ( [\"tasks.assignProjectAgent\"] ) , attachments","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.attachments",". has ( [\"tasks.attachments\"] ) , watching","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.watching",". has ( [\"tasks.watching\"] ) , doneSinceLeft","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.doneSinceLeft",". has ( [\"tasks.doneSinceLeft\"] ) , commentUserNotifies","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.commentUserNotifies",". has ( [\"tasks.commentUserNotifies\"] ) , listPaged","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listPaged",". has ( [\"tasks.listPaged\"] ) , listSummaries","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listSummaries",". has ( [\"tasks.listSummaries\"] ) , filterChoices","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.filterChoices",". has ( [\"tasks.filterChoices\"] ) , counts","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.counts",". has ( [\"tasks.counts\"] ) , lookup","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.lookup",". has ( [\"tasks.lookup\"] ) , listWatched","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listWatched",". has ( [\"tasks.listWatched\"] ) , listAttention","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listAttention",". has ( [\"tasks.listAttention\"] ) , listLinkedWorkspace","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listLinkedWorkspace",". has ( [\"tasks.listLinkedWorkspace\"] ) , listTracked","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listTracked",". has ( [\"tasks.listTracked\"] ) , listReviewMode","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listReviewMode",". has ( [\"tasks.listReviewMode\"] ) , listOrder","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listOrder",". has ( [\"tasks.listOrder\"] ) , listReadiness","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listReadiness",". has ( [\"tasks.listReadiness\"] ) , listExcludeAssignee","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.listExcludeAssignee",". has ( [\"tasks.listExcludeAssignee\"] ) , bodyPrecondition","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.bodyPrecondition",". has ( [\"tasks.bodyPrecondition\"] ) , waitingOn","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["tasks.waitingOn",". has ( [\"tasks.waitingOn\"] ) , }","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["compaction.harnessThresholds",". has ( [\"compaction.harnessThresholds\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["conversations.settings",". has ( [\"conversations.settings\"] ) , reset","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["conversation.reset",". has ( [\"conversation.reset\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["github.repos",". has ( [\"github.repos\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["messages.context",". has ( [\"messages.context\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["threads.postOperations",". has ( [\"threads.postOperations\"] ) , attachmentChunks","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["thread.attachmentChunks",". has ( [\"thread.attachmentChunks\"] ) , leanActivity","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["threads.leanActivity",". has ( [\"threads.leanActivity\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["branches.finishDelete",". has ( [\"branches.finishDelete\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    [
      "push.registerKey",
      ". has ( [\"push.registerKey\"] ) , revokeKey",
      "13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"
    ],
    ["push.revokeKey",". has ( [\"push.revokeKey\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["agents.createdBy",". has ( [\"agents.createdBy\"] ) , projectWorkers","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["agents.projectWorkers",". has ( [\"agents.projectWorkers\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["project.update_source",". has ( [\"project.update_source\"] ) , syncBase","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["sources.syncBase",". has ( [\"sources.syncBase\"] ) , syncBasePath","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["sources.syncBasePath",". has ( [\"sources.syncBasePath\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["workspace.measure_sizes",". has ( [\"workspace.measure_sizes\"] ) , setLocked","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
    ["workspace.set_locked",". has ( [\"workspace.set_locked\"] ) } ,","13e4c11aa25c1d370e07272e6fd331775a1e8e43a28d732712d0321f236941f1"],
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
    ["files.hints",". includes ( [\"files.hints\"] ) ) ;","696d73df4befe57fd7f675bf2f6987edd81138ce7ab50f6fc79994f47cc2f3ba"],
    ["writes.confirmed","( [ [ [\"writes.confirmed\"] , dispatchWriteConfirmation ]","7d1824116f67f0f38b9e63b9075e5815ffe18dd3c1abf9c0594e78a8921fbde6"],
    ["models.changed","] , [ [\"models.changed\"] , ( _payload","33ace9748a13b4576ffe10b64014d642396cfb0f1a542e1a4b41fd9e51122e9c"],
    ["harnesses.changed","] , [ [\"harnesses.changed\"] , ( {","33ace9748a13b4576ffe10b64014d642396cfb0f1a542e1a4b41fd9e51122e9c"],
    ["compaction.changed","] , [ [\"compaction.changed\"] , ( payload","33ace9748a13b4576ffe10b64014d642396cfb0f1a542e1a4b41fd9e51122e9c"],
    ["secrets.changed","] , [ [\"secrets.changed\"] , ( payload","33ace9748a13b4576ffe10b64014d642396cfb0f1a542e1a4b41fd9e51122e9c"],
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
  "core/httpSession.js": [
    ["writes.confirmed",". type !== [\"writes.confirmed\"] && frame .","3b24f398d622d2aa8992d6ad855350470b4dca2fff709938cc6670e90e8f5ebc"],
  ],
  "core/peerLink.js": [
    ["rtc.diagnostics","] , [ [\"rtc.diagnostics\"] , receiveCandidateDiagnostic ]","4448b1282477bfb9c6ed1d5b46934f4b21fb358494a5cfdb2c0e66a5961690a3"],
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
  "core/directoryPages.js": [
    ["files.hints","=== true ? [\"files.hints\"] : \"files\" ;","e0b43d221518e8f485d3f8538fbc8d3f53d0610feb46a530cc29cc86a3e6b748"],
  ],
  "core/directoryWatcher.js": [
    ["files.hints","deviceId ) === [\"files.hints\"] || shown (","d20875409ea449d811a8abcb8b700d9af29e7dc6d8326c443d3a203f2f42638a"],
    ["files.hints",". includes ( [\"files.hints\"] ) , }","80ce1e7567d7b35f7f88f16ce06c4516c6f428a377448e5498128906bd83b824"],
  ],
  "core/taskReviewFiles.js": [
    ["files.hints","deviceId ) === [\"files.hints\"] ) { const","e1a8c5fdf46a4e625590ae2fb4e03799d5dd878b7f652502adc904229b07e8ad"],
    ["files.hints","deviceId ) !== [\"files.hints\"] ) return legacyLiveChanges","710cab5b81667a384a13f72b7d79a77d4543a5885ac47cb09183e57c2b6f8fe8"],
    ["files.hints","deviceId ) === [\"files.hints\"] ; const target","8b39c2ece19ccd6b906b88a1b2d51c6af5dde27e9d81e44392cf41cb7ebc5e49"],
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
    // writeOperations.js compatibleRows: capability check before quarantining an incapable bridge.
    ["bridge.epoch","?. includes ( [\"bridge.epoch\"] ) ) await","7c787332c8e6015f1f2ed010aadd1ebeb550f2a72eb52c8ef56a130374268a4a"],
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
// Each key pins the entire named spa/src file, including assignment edges.
// #1009: download adds scoped fs.read (whole/paged); directory actions retain
// NEW_ENTRIES and the existing upload RPC wrapper/queue. No new route or
// dynamic method producer. #1008 changes preview classes only.
// taskReviewActions/Feedback now delegate to the inventoried taskReviewMerge/Batch
// modules; neither delegating view has its own RPC/producer occurrence.
// #894 finite metadata/call-edge audit: docs/task-query-cost/http-resource-coverage-audit.md
const sourceFingerprintParts = {
  "app.js": ["e9b93f5b", "01a713dd", "1c1f8ef7", "00229053", "d2bf2d3a", "b372d574", "81aaff15", "71dc3f58"],
  "appState.js": ["d24d9edc", "6e4988f0", "8a5289ee", "6c449b3f", "91bdbe42", "7cd43ada", "7a9df6a4", "acd39971"],
  "connection.js": ["009f7235", "cbcbf300", "43c93b7c", "bcfb2243", "e17e2797", "d809e8af", "5e60cb14", "d0fe86f7"],
  "core/activityRuns.js": ["2bd80d81", "ad2ddde9", "da2fc194", "95b7b7ba", "4d0a86f4", "29a52906", "009b6fc5", "5221d710"],
  "core/adoption.js": ["350b90d5", "9e535f4b", "15adf2ef", "7eacd8c0", "fb946020", "b6130cb8", "ce729fed", "bb90597e"],
  "core/agentDefaults.js": ["f856d23d", "2c446c14", "c6e4d78e", "e11e737d", "75e1fbc1", "fa79564d", "2ef20a2b", "1cdbbcb0"],
  "core/agentModes.js": ["5764f59d", "200944fd", "e1fb485f", "a1b58c05", "633b7f0f", "f9b7cbcd", "4e62b9db", "31bcb3a8"],
  "core/agentRail.js": ["f085acd7", "ccc8371b", "d3377115", "758c5ef0", "439246d2", "c1d2aad1", "5672225b", "316cd6d7"],
  "core/agentRailContext.js": ["e175542d", "376171c6", "e04b61df", "847af2cf", "a3a2c56e", "8fbeb913", "9593b4cb", "50809f5c"],
  "core/agentRolesPanel.js": ["c05b7c96", "5849ec7f", "0d258bc2", "96a7795e", "2a4663f4", "57e78316", "de145b90", "699b4a91"],
  "core/agentSurfacesModel.js": ["546cf5aa", "3a478c63", "4a0f02f5", "4eafcae5", "53a5ffd3", "3ad4f563", "c075aa93", "dcbbef9d"],
  "core/bridgeApi/v1/compactionParams.js": ["506db6e2", "f8148bc4", "925e889b", "61c6fe15", "d8cf74e2", "ef7e468f", "b610115f", "7132ce5b"],
  "core/bridgeApi/v1/index.js": ["a348bca9", "4248bc30", "4fd53ed2", "2de89d0a", "0ab7ab83", "e20075a6", "b90e947f", "e64c1f19"],
  "core/bridgeApi/v1/writeMethods.js": ["f311847f", "da193503", "678520ec", "e289e013", "fbbcc9f9", "26f2aae9", "32c184fe", "7f27b03d"],
  "core/bridgeEpoch.js": ["e04a0a11", "6535b020", "abcac20c", "972a554f", "8d51fbb9", "6ef07fd8", "16062ed0", "9cd4d213"],
  "core/bridgeUpdatePanel.js": ["f9b19cc7", "4abdfc5a", "d159194d", "49ee91ed", "d39ab2e7", "15a6a80f", "0cb51530", "623ca8e5"],
  "core/bridgeUpdates.js": ["279cb932", "0495aa46", "c347f93b", "10a492d5", "8d29b0a9", "e9309a9a", "91de7993", "e5f40ad3"],
  // #997 runtime-board cacheSync.js:754,783,1978: existing board.list also refreshes only runtime collections; durable scope/tip writers stay gated, no new method.
  "core/cacheSync.js": ["e05ccac5", "8746f47f", "e98780dc", "03a41d50", "fa3df544", "909abbba", "4fbf7dd7", "76ec7b0a"],
  // #997 runtime-board captureDecisionView.js:142: branch choices join cached runtime run bodies; existing RPC and mutation dispatch unchanged.
  "core/captureDecisionView.js": ["15299f0a", "f1762e74", "8af22269", "0f16e798", "ccf8e367", "d2d759e2", "554ce8fc", "002692f1"],
  // #997 changeEvents.js:832: pass the accepted greeting version with existing compaction invalidation reads; no new RPC method.
  "core/changeEvents.js": ["934d11bb", "09dc8caf", "bd353109", "2e2a242c", "2af42dea", "e91daf72", "89963624", "1870a496"],
  "core/changesComments.js": ["7a5e0b06", "c47bf122", "ac5370b8", "debd8dc8", "388227b8", "f9ea1623", "d413134a", "635a2a7d"],
  "core/changesModel.js": ["1a2a1485", "24c755c4", "706c0111", "981a4765", "2cc7236e", "4ff87fce", "4a7640ba", "13a75672"],
  "core/changesReview.js": ["efa3d8fd", "2f936aea", "3c017fef", "5e7e463e", "5b6485cf", "abc5c809", "6e9a2636", "42ac75ec"],
  "core/chatChoiceController.js": ["d257b6c5", "f32d9067", "226af14c", "bd8ec71e", "718bf188", "1e994ef1", "66edcceb", "4499fa61"],
  "core/chatRepository.js": ["131aafd6", "9d9e29b7", "defb8e63", "f70c3dc3", "3820671c", "541f4adc", "a8a08418", "7dcfa920"],
  // #997 compactionCache.js:57: use captured version authority without importing connection state into cache helpers.
  "core/compactionCache.js": ["4e4482a7", "24057b98", "0a358b45", "3beeb686", "8c8c7388", "79aecece", "7b4cc21b", "75595548"],
  // #997 runtime-board compose.js:236: branch options join scoped cached runs; producer method maps and RPC arguments unchanged.
  "core/compose.js": ["04ba4233", "c03d8a65", "2fb7e1a0", "33dd7876", "b9eebdbe", "8f295ce8", "d3407278", "65247a74"],
  // #997 runtime-board composeView.js:274: branch choices use cached runtime runs; RPC and mutation dispatch unchanged.
  "core/composeView.js": ["c23f86ee", "e1c3714c", "7d01aa7b", "6178100a", "291f1c3e", "44eed077", "6f4dedc9", "a52c7553"],
  "core/consoleModel.js": ["4feba529", "663ba471", "6fe6e2a1", "3eb66e5c", "815545f8", "f70a5b48", "a6248d75", "94d9ed20"],
  "core/conversationCompaction.js": ["4e076445", "2fd7fb1d", "1ca9681e", "b39ea814", "01d5b9f9", "26f91118", "a52788ca", "54220e9e"],
  "core/conversationDetail.js": ["a54ac9f8", "f54c61d4", "cea363a5", "b098964f", "16ecc016", "490b853d", "76d94602", "ad5a773a"],
  "core/createWork.js": ["42444ebc", "8ae4117e", "ba2cdde6", "7d740ba3", "1850d7a6", "a63c9fd2", "50784964", "18d57196"],
  "core/defaultHarness.js": ["b7ed5ad6", "9fdfdc35", "84dd803d", "3c0e95fd", "72904016", "4cb29938", "97723e94", "2ace812d"],
  "core/deviceContexts.js": ["4e9c3990", "7dee88ec", "810274f7", "2defd5bf", "c24f8902", "bf38503f", "54f61680", "b4cf4fd1"],
  "core/diffSort.js": ["9ef743b8", "16f0a5ae", "1d9ec36a", "455f0e76", "c04a0902", "8deca6c7", "70acd150", "dbe96ac6"],
  "core/directoryRail.js": ["98080c38", "f2152285", "3e025868", "10652120", "d12c8ead", "cfa571c6", "6c3476c8", "b6b56ef2"],
  "core/fileDiffs.js": ["708cce52", "81bbdc00", "82791bce", "f9d05524", "b3b3b017", "d283e106", "e977da82", "b3b2707e"],
  "core/fileDownload.js": ["e53778d8", "5f5c1cd3", "b3911bd3", "f6077ecf", "e0ca80d7", "a7c15c52", "6a9e47fb", "0a3df067"],
  "core/filesUploadActions.js": ["0b4425a7", "5323bc95", "a96d1ae2", "32a48f65", "0e2c09e2", "d145483b", "ed4a2978", "f9ee6cc6"],
  "core/filesUploads.js": ["f0f92dd6", "99cd5370", "8978158a", "9456ca4c", "3f685a13", "466ef521", "a055e412", "160ed312"],
  "core/fileUploadRpc.js": ["7891a6ca", "183a9b67", "b7fe41fc", "ba71c70a", "1896ec75", "2809155c", "1375617f", "a9ab96d0"],
  "core/fileUploads.js": ["a1a4c2df", "5ef03503", "483e7b99", "099e04a0", "77c9e865", "a22a23dd", "a704cd79", "6678f239"],
  "core/fileUploadsModel.js": ["d11fdd79", "8bb9318c", "fbc2cff2", "29b16930", "9d73b3ee", "89d5d976", "7e2b756a", "69d40aac"],
  "core/githubRepos.js": ["4526fb16", "bfe42e0f", "57b963ea", "fe623873", "0fe9ccce", "521ecc83", "b9117e59", "af64462f"],
  "core/gitPane.js": ["3b9c66d1", "186e1669", "3ecc46b0", "0afe28d8", "10e87dd3", "55dfbb00", "44fa79f8", "88d71409"],
  "core/httpSession.js": ["d126ceab", "0dd1f2ad", "df3cd6a9", "0af4113e", "228a2843", "14eb6c04", "36b84eb3", "0c8342b1"],
  "core/inboxCaptures.js": ["48a7f5fb", "b2132a83", "5901fb31", "4fb0c5ae", "97005297", "2d0b9db6", "be2c3e6d", "1271b663"],
  "core/inboxDevices.js": ["f9a293e4", "dff4b2b4", "f5fc7940", "f5c1f48d", "a66a5718", "238826a6", "e3dc6b9f", "4060a79e"],
  "core/inboxSeen.js": ["8131bfc1", "8d81301f", "e91c3d80", "44cb2dec", "5c60d797", "eb8516a4", "3b49d94a", "ca32676a"],
  // #997 runtime-board inboxView.js:193,206,320: scoped pending display and cached branch choices; existing RPC and mutation dispatch unchanged.
  "core/inboxView.js": ["73e318f8", "d3681787", "0ff602b2", "03923c2e", "8ac8746f", "13fd8ee3", "d314907b", "c5aa4b97"],
  "core/isolation.js": ["1b646651", "086eea7f", "b0a97d6f", "8993ed77", "7e6e246e", "4110fbc2", "7defb076", "77f79221"],
  "core/markdownFences.js": ["10057396", "68214f4d", "d193015e", "2552698e", "c48313df", "6df2e223", "7e3bc837", "2805a4bb"],
  "core/modelCatalog.js": ["c293ea11", "fff0969f", "7852c8cd", "0c252f82", "fe666159", "95919417", "25a661ec", "c93fd9b4"],
  "core/navigationVisits.js": ["f8d32f7a", "6bf64af6", "81866475", "02ebd8fc", "3a0be61c", "c9f0d9f3", "e1cc0c06", "dd4dc05a"],
  "core/optimistic.js": ["664afc7e", "9083408c", "39d1e085", "5a531484", "9d315ad7", "dcbe2655", "b119e508", "6f3ef787"],
  "core/pathProbe.js": ["1518fd69", "692eec10", "f8018014", "c81c6ee1", "4edda84e", "efee05d6", "472437de", "73c96fea"],
  "core/peerIceQueue.js": ["0cd05f87", "5def0e05", "b95fd35a", "3498571e", "7a93f83e", "8da1d594", "8022f75f", "0870be3b"],
  "core/peerLink.js": ["07cce688", "624c9a99", "9dd6a3d4", "77c8ff8f", "5a325998", "0a67eee0", "a0793c5f", "d8d0e597"],
  // #997 projectAgentsCache.js:57: load runtime authority only for Web Locks reads; live callbacks fence upgrades without import side effects.
  "core/projectAgentsCache.js": ["a7c5f4b5", "321ecafb", "b30fd292", "07302c6b", "1f62638a", "c16708ff", "94bfaa24", "4e46db17"],
  "core/projectAgentSetting.js": ["cf7d410a", "f3bc1eb4", "1b4e7833", "9dc4cf50", "23b01280", "01636e27", "72e8edb1", "e4a7e379"],
  "core/projectChangesRpc.js": ["613fe381", "59f4a3a4", "08a518ed", "4e535261", "21ca4a58", "5e5011d9", "c0acd52e", "bb8e5014"],
  "core/projectConsole.js": ["b44678cc", "4c2dbc2a", "188695a2", "307b4883", "4d4fb733", "091622af", "3c648066", "9d1e32bb"],
  "core/projectFilesRpc.js": ["e84ae7b7", "ceffb0cc", "7ff4c6cd", "cfc0c05f", "13a0d831", "079f3b3e", "5dae2e56", "ce4686ac"],
  "core/projectTerminalRpc.js": ["b391c6dc", "aa385e26", "aadfb8b8", "f6ef5cc9", "9a8e7a79", "d9d757bb", "cdd1356c", "867cadea"],
  "core/pushKeySync.js": ["ecd9373a", "3f2dd8cb", "af448d18", "1098ca18", "ef83c7bb", "923a9c90", "b5d5f042", "9ac75438"],
  "core/railMode.js": ["9a3738d3", "e4361bdf", "fef94f86", "f803203f", "efbe6cbf", "8513e3f7", "5fc7602c", "dd4e5e6b"],
  // #997 readStateRepair.js:96: decimal read repair retains existing board.list and project-roster confirmation methods.
  "core/readStateRepair.js": ["c374e7a6", "23e8215a", "1c4a16b5", "337a4bf2", "32babbcc", "246bfc57", "68c3cdfc", "5e916be7"],
  "core/revisionBodies.js": ["60606411", "ec3a6f2c", "7c923581", "f22c39cd", "43dd8a04", "69ff5c40", "e4e490cb", "e3119193"],
  "core/rtcTerminalLink.js": ["9e8a6c70", "266666d9", "45b2f045", "4f29f5fc", "6293c562", "d2f0ed25", "15a05fd6", "a43dd438"],
  "core/scopedCompactionEditor.js": ["b5d366f5", "90a2be6a", "b5c01379", "ccad5198", "7298a4cf", "bdcbdc88", "2e022cd4", "847d6330"],
  "core/scopedProjectColumns.js": ["050be4f1", "7bc93888", "92cfb0a6", "eec46373", "f5b4c3f8", "22bbd079", "a8a0081b", "cf9db94a"],
  // #997 scopedSyncDiagnostics.js:4: device is a closed diagnostic label, never a resource selector.
  "core/scopedSyncDiagnostics.js": ["6cc8fdb6", "b09c4e34", "fe7d1304", "b21f55d1", "28d9740b", "71c566cd", "db179dcd", "ef3bd1bf"],
  // #997 review scopedSyncEngine.js:62,147: declined publications retry bounded recovery through the same three sync queries; no new dispatch site.
  "core/scopedSyncEngine.js": ["2f3ab7a1", "b5bad14b", "d4f774c0", "dbbec9c6", "124d8d1d", "05076d00", "a441b99d", "2b2a333e"],
  // #1005: staged-secret draft metadata and cache reads use explicit secrets.* methods.
  "core/secretDrafts.js": ["01e542b2", "d8c57354", "063745c8", "2080e155", "a6da123d", "385ff991", "4c59203c", "17e171c0"],
  "core/secretMetadataCache.js": ["06759ad6", "c728c4c1", "2c49dcb7", "bff92033", "da9b45f7", "99cccfe5", "496164ab", "3c96c0e1"],
  "core/session.js": ["3d180500", "f13f38ec", "64d5d01d", "ec44e3c7", "ef80b3cc", "85470f82", "ad7080e1", "69c47137"],
  "core/sessionRpc.js": ["5d583ef2", "0faaf478", "09fbf975", "ab15af9f", "a78fd5f8", "fc4bab2e", "49d4ef64", "4e13ad2d"],
  "core/sessionSwitch.js": ["dc232a71", "5b006195", "d7910b8e", "8c59e611", "a36dd264", "8bbcfcc8", "0d8739f3", "371accc7"],
  "core/shell.js": ["0f5d52c0", "19499900", "de686569", "6e687a5e", "abf49c6a", "8801b17a", "a3446163", "1a1df469"],
  // #1003: bounded secret metadata reads use the shared SQL admission lane.
  "core/sqlReadCredits.js": ["6c9b5955", "301a1704", "ef9798df", "3853302b", "edb5b267", "872386a6", "e8e0f30f", "8689421c"],
  // #997 syncDiagnostics.js:14: the device label extends diagnostics only; RPC verb inventory is unchanged.
  "core/syncDiagnostics.js": ["699d4595", "41400002", "6be4a4de", "45c68a8d", "f59ab913", "b68f7991", "936cad7b", "48ee7871"],
  "core/syncLists.js": ["e44aee99", "346eb027", "1e6486f1", "dac9a7ac", "9acdd7b8", "1c4a00f4", "ce196d01", "e22f5292"],
  "core/syncSuccessors.js": ["8f215712", "252114ce", "b5f282bf", "3b818ded", "2bfe5a52", "60ced14f", "965e228e", "9af31a83"],
  // #997 syncTaskPhases.js:25: task pages keep existing reads while session publication yields to the device owner.
  "core/syncTaskPhases.js": ["594c72ca", "cff8fa97", "cce40760", "49a7754d", "5eab461d", "41cd2789", "92bf5cfb", "0d28ab42"],
  "core/taskAggregateReads.js": ["145d7d37", "d5b65ef2", "fca15f4d", "c61a586c", "f62233a0", "b8b309eb", "4a69f9d4", "8575b030"],
  "core/taskAttachmentBodies.js": ["7acfa90a", "4413c7df", "89207db8", "07fa4c16", "b21cc95f", "dc3bf412", "eb0c14de", "671d3006"],
  "core/taskChecklist.js": ["405a6444", "8132e330", "6ff6c056", "a1e71101", "3bdd9028", "a42eae55", "d6ebaf5b", "952f8a34"],
  "core/taskComposer.js": ["44233e6a", "d60f8251", "f724f7bc", "af16b319", "2c205f31", "5a10e50b", "b233a25f", "ed90f8ad"],
  "core/taskCountReaderRuntime.js": ["dc823412", "765c408d", "09a4bd9d", "959b3450", "116e39aa", "fb67622c", "6afd9df6", "ce9e3eff"],
  "core/taskFilterChoices.js": ["6e7fca24", "b96c853f", "5baa71d1", "9e8c17fc", "9a8ad6ff", "14e53667", "9d583cd0", "032ce3ee"],
  "core/taskMutationFence.js": ["1f7ac815", "7e25b93b", "78989130", "90e22a43", "b6c956a5", "2cb5f2a0", "e424fb92", "d8c7cd29"],
  "core/taskPaneSummaryReads.js": ["38940784", "0edcdfa7", "9b24a843", "58a0edf6", "142d6012", "742d2793", "d3f0c487", "d74a2d8e"],
  "core/taskReferenceLookups.js": ["d96512cb", "87a770a1", "4a0ecbe7", "c26db1f1", "e7ad9481", "284534db", "db18cad9", "3cc292af"],
  "core/taskReviewBatch.js": ["fc2f7cbe", "2ead7274", "41b47492", "2284e88a", "57cfa35f", "c5247a57", "2d78ccd0", "49e11c3e"],
  "core/taskReviewCache.js": ["a8dd24bb", "d7fd739e", "5a3d7934", "1adb99bb", "daaeffe6", "58cdc0aa", "ddf24d77", "12edcaef"],
  "core/taskReviewChanges.js": ["b0e3e23b", "b0716f2d", "b4bd806c", "8ee1fb29", "04199fbe", "3f8f19dd", "56f4b1df", "946865bc"],
  "core/taskReviewControls.js": ["0663873f", "45fd81a3", "c5c76f0d", "5fc39eff", "d5554bb3", "3a412be3", "3e866b7f", "fba967a3"],
  "core/taskReviewFiles.js": ["83db28dc", "66ec3f81", "1bf00bfe", "d2e1dc7b", "7b9ff81a", "04b9cc23", "26f9f9bb", "e21df135"],
  "core/taskReviewLifecycle.js": ["78fb3824", "b3ac9dfd", "66316264", "05b68766", "b9e58ec0", "bb6d661a", "fc072202", "b23e50f3"],
  "core/taskReviewMerge.js": ["3f945ae3", "9fc65fae", "c32e2d92", "dbfd041d", "e5824be6", "f4f4f84c", "cb641669", "432efdb6"],
  "core/taskReviewPage.js": ["54c670db", "1b5a28a6", "9cf7f74d", "6201af87", "fd884931", "7e7ca3cc", "da1884b0", "5c2af37e"],
  "core/taskReviewReads.js": ["b8f024e8", "a779eb28", "3bb33ef2", "18fae770", "3dbbca9b", "151b90f9", "77b248ef", "05d307c0"],
  "core/taskReviewReclaim.js": ["39f9773b", "9fb74944", "f19e9d44", "8698c30d", "cbd907a0", "bde50825", "b8601ff9", "a2100184"],
  "core/taskReviewSupport.js": ["7662abfe", "2c9571c8", "e80a8333", "664118b2", "8979e6ea", "850fb345", "579648fd", "73103bee"],
  "core/taskSummaryReader.js": ["a9ed4098", "48ff6f48", "9c6a8e42", "a99e8210", "e43a6187", "b3795253", "25a4a110", "65591e0c"],
  "core/taskView.js": ["547b447a", "1b089702", "8da986a0", "97cbffed", "86cd8be0", "dfabf680", "23671d54", "6ac7ef11"],
  "core/taskWaitingOnActions.js": ["576156a7", "454cbd39", "333d8645", "9f62ff9e", "00e96c90", "55eb5f60", "441927a5", "f6cf4597"],
  "core/taskWaitingOnPage.js": ["b5a78897", "c7cfc8dc", "8a1ad2e0", "c77cad0a", "874280e0", "1b394697", "37da2547", "7a1c3636"],
  "core/theme.js": ["5f821f5d", "933be0fa", "c04dd60c", "8ed91a2a", "4fd56dbb", "ab2c8e04", "e86ee7f5", "60d8e2a6"],
  "core/threadAttachmentPages.js": ["df0b246c", "a850af5a", "c612aeac", "414feaa5", "03db6170", "9511cd9c", "372279a6", "9c417822"],
  "core/threadSync.js": ["bf280ad8", "20d158b6", "aaa4f7b9", "695d3816", "1d0200e6", "cf6756bd", "bc0e1372", "2e452737"],
  "core/trackerPages.js": ["2f749a25", "259d2e7b", "904e56ef", "15e91197", "da83450c", "c490c89c", "12b5ae68", "4b7a60e3"],
  "core/trackerTaskPage.js": ["e3e7d0ff", "2db22922", "ffcf0889", "5440177c", "ccb64ccd", "7d16b9b6", "a19243a5", "f5985960"],
  "core/trackerTasksPane.js": ["80f5f62d", "fd719fe1", "535f6317", "215df125", "eeda25ec", "ddcbf174", "924f173f", "99ae4b57"],
  "core/userPresence.js": ["b4a43148", "8352b41e", "f38b7e72", "b03317b3", "ca411d39", "6e7fb659", "b308effa", "511a43aa"],
  "core/watchedTaskFollower.js": ["d08812f3", "b1c71fd1", "c45cc354", "8e0d2495", "ad2f20de", "f54548b0", "89d31f41", "17aa347f"],
  "core/watchSetting.js": ["550f0da2", "883ae225", "24fd912e", "e669ffb0", "c1d76f0b", "3879059c", "c35a16c8", "8d6065c6"],
  "core/watchToggle.js": ["0c994a20", "9e96598d", "45a92ac1", "3e8a83f5", "95ceaa15", "a98a3720", "f3cad429", "79b9fc50"],
  "core/workspaceDefaults.js": ["26caf0ff", "77dcee98", "234785f1", "a7858438", "d1599850", "87f6f3f6", "90d71240", "2d106714"],
  "core/workspaceGitInitialization.js": ["2b018fe5", "248c6586", "27051b5b", "6a7f6f9e", "daebd529", "68ef1bc8", "af4ea4be", "ae2c1581"],
  "core/workspaceLifecycleSetting.js": ["15c495c8", "5c146e7c", "3e1d376b", "eefb5dbf", "356abf02", "b879315f", "815c5a8a", "123b1b54"],
  "core/workspaceLock.js": ["8464b243", "84658e25", "308e37b8", "ac31b13f", "f30ebad2", "fe08326f", "b831630e", "d650d6fd"],
  "core/workspaceRefPicker.js": ["8e52ee89", "75a29f02", "4ede31a8", "68cbfb6d", "dbc1c227", "0642e2a9", "a7dd1729", "4503b2bc"],
  "core/workspaceReviewEntry.js": ["a74d65f1", "789b793e", "3b584c30", "8085429c", "708f60fe", "54e1cd19", "e32cb0be", "8bc72509"],
  "core/workspaceReviewForm.js": ["c73939fb", "cb195111", "85819eed", "9716f89b", "6a8f72bb", "026af5b9", "6dc154e9", "e67e42b5"],
  "core/workspaceReviewTasks.js": ["0ce927c3", "057199b7", "8c84154f", "aee738c5", "02391da6", "be84ac08", "d1190908", "5073f226"],
  "core/workspaceSizes.js": ["5906d4c2", "fda13846", "3542c664", "26018741", "0bf741e3", "1a10dc8e", "e5feb8ee", "9c08416d"],
  "core/writeOperationCache.js": ["9da4e187", "2b41988a", "a54e493e", "ddeb5d8b", "40288fd9", "7b4b4a3c", "1f1a816f", "c8847f4b"],
  "core/writeOperationEffects.js": ["d30332d1", "89b12c79", "150580d0", "aede7e2b", "d2e23499", "d6003a79", "f13412fe", "b895c728"],
  "core/writeOperationIntent.js": ["5476394a", "087dc5ce", "5c71e673", "8b977ba3", "e665bf53", "fa9529be", "78bf2c38", "48f27991"],
  "core/writeOperationRefresh.js": ["b5d241f8", "64bba629", "58a8bbab", "6673dafa", "0c5490bc", "2e70cbc9", "d0048bf4", "20e4c528"],
  "core/writeOperations.js": ["481c19c9", "ae1ee5a1", "f4fd6d5d", "5b18407c", "247e31a0", "d6052460", "338b6ed7", "3cf163ac"],
  "push.js": ["c7a3eb27", "240e0a4d", "3e314aee", "369bea39", "25bf83e0", "f105e41a", "0e4ed716", "4e53028b"],
  "sheets/browser.js": ["e226c6fb", "9689c6bc", "39ba311b", "335481c7", "f444e696", "36c5c8b6", "013588ec", "bd2a4dbe"],
  "sheets/newRepo.js": ["e1addf8b", "abc706f6", "ca74fa24", "7e1f3208", "aae3d460", "37157135", "21e46d2f", "f187cdfa"],
  // #997 projectSettings.js:232: the existing project.list loader delegates to owned device recovery.
  "sheets/projectSettings.js": ["1e7423ec", "47cd628d", "c76970e7", "fca5da5f", "c623809c", "c51769ab", "1b172bb1", "906737ee"],
  "sheets/projectSources.js": ["f9eb9e2b", "9c526294", "46090552", "02740c7a", "25e0f849", "8162d486", "fd90c590", "05524d9d"],
  "sheets/sourceSyncControls.js": ["a0c9eb24", "846213e3", "1cc5b961", "8058d449", "6a46f577", "2b154112", "724dc4e5", "6041f096"],
  // #997 workspaceSettings.js:292: the existing project.list catalog loader delegates to owned device recovery.
  "sheets/workspaceSettings.js": ["5cc470f0", "7f95fc28", "02c7bfb0", "478c4510", "e33b5183", "1e6be057", "6928f74b", "82185347"],
  "terminal/manager.js": ["4027a8b0", "af931af9", "3d71d687", "c930e06b", "bd55b938", "ad0db4da", "e20dc150", "60ed795b"],
  "terminal/session.js": ["ce4e3036", "ac5b84b3", "ae7583e9", "2b940e62", "4dfe75b7", "d77a06e7", "dfda65ca", "8c8ede4e"],
  "views/archive.js": ["6f4a7962", "88f34292", "73eafead", "5dc6229b", "5b040b7a", "09baf137", "8cbbce83", "6c83d400"],
  "views/branchView.js": ["bd60fcf4", "7c212e82", "4972bdd1", "5b4a3563", "54913179", "782e8a54", "ea8d0a73", "30816a7d"],
  // #997 deviceProjects.js:82: the existing project.list loader delegates to owned device recovery.
  "views/deviceProjects.js": ["8ff87af9", "bead4587", "20c4bc9d", "89c548aa", "9a137b45", "4cb5741b", "880a3013", "429a0844"],
  "views/deviceSettings.js": ["ff192f53", "496b85f6", "50f01429", "9a797f58", "443cd757", "79b5ca5f", "9ba81fbd", "aa8d1847"],
  "views/files.js": ["bc4913c5", "865b9dd0", "e28f655f", "242c9011", "456a4139", "b2cfd372", "6498e15b", "f3116261"],
  "views/projectChanges.js": ["e195b594", "3a7689fe", "14bfc899", "18ada18d", "ab36cd52", "ad96ba97", "a877c4da", "c9c3b78f"],
  "views/projectView.js": ["b790e1ad", "c92e3880", "d880307e", "45527ad4", "70eade31", "b21f74d0", "423e637a", "6999aa4e"],
  "views/taskReview.js": ["7d540d05", "70beae35", "200434ea", "decae581", "f1b126d6", "9a38fe38", "642a621a", "2e34a257"],
  "views/taskView.js": ["483fdb16", "52a51dd3", "ea9f60d0", "c21f16a9", "090b27a4", "4839602c", "4653bfcc", "38959753"],
  "views/workspaceView.js": ["a8297151", "b9889eea", "1ef6492b", "4242340b", "21deba98", "2afe3f42", "4f4be235", "4268410b"],
  "views/worktreeReview.js": ["85ee8f63", "e33ae091", "1057ec2b", "8bd416bd", "d4b709ba", "71953ecd", "05807b6d", "ee80f4db"],
  "core/directoryPages.js": ["1114bcb9", "d7703490", "4542e5ef", "d06e4f25", "b52d6e2e", "4c206d81", "9586fd6a", "92ccc080"],
  "core/directoryWatcher.js": ["3016199e", "f2ebe782", "5731fa24", "665dba3a", "6fd96a9e", "f5a4ee11", "c6841b70", "db07db4f"],
  "core/taskTimelinePages.js": ["da2840fb", "5992d7ce", "038bbc4d", "9c01a8ab", "c320a6f3", "7592e5be", "d5cbf08c", "4fcf8220"],
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
  // cacheSync.js:2465 readReviews forwards tasks.review.get/diff (in addition to the existing read adapters).
  "core/cacheSync.js": [
    "ask ( request . method )",
    "callWhileActive ( method )",
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
    "context . rpc ( ... asked )",
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
  "core/httpSession.js": [
    "adapter . call ( method )",
    "call ( method )"
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
  // syncTaskPhases.js:61 asks refreshScopedProjectColumns to read tasks.columns.
  "core/syncTaskPhases.js": [
    "ask ( method )"
  ],
  // taskAggregateReads.js:121 dispatches contract.method: tasks.counts (:132) or tasks.lookup (:140).
  "core/taskAggregateReads.js": [
    "callRpc ( contract . method )"
  ],
  "core/taskChecklist.js": [
    "producer `build.taskChecklist:${JSON.stringify([deviceId, projectId, taskId])}`"
  ],
  // taskCountReaderRuntime.js:31 forwards createTaskAggregateReader calls to tasks.counts.
  "core/taskCountReaderRuntime.js": [
    "context . rpc ( ... args )"
  ],
  // taskFilterChoices.js:133,143 call the local pagination helper; :106 sends tasks.filterChoices.
  "core/taskFilterChoices.js": [
    "ask ( next )",
    "ask ( { ... next , cursor : held . next_cursor } )"
  ],
  // taskPaneSummaryReads.js:70,108 refresh/page the filter choices reader (tasks.filterChoices).
  "core/taskPaneSummaryReads.js": [
    "ask ( reader )",
    "ask ( reader )"
  ],
  // taskReferenceLookups.js:164 forwards createTaskLookupReader calls to tasks.lookup.
  "core/taskReferenceLookups.js": [
    "context . rpc ( ... args )"
  ],
  // taskReviewBatch.js:155 sends operation.method, assigned tasks.comment at :42.
  "core/taskReviewBatch.js": [
    "callRpc ( operation . method )"
  ],
  "core/taskReviewCache.js": [
    "mutate ( verb )",
    "mutateRpc ( `tasks.review.${verb}` )",
    "producer `tasks.review.${verb}`",
    "reviewMutationRpc ( scope . deviceId )"
  ],
  // taskReviewChanges.js:228 creates the adapter used for tasks.review.diff at :242,288,306.
  "core/taskReviewChanges.js": [
    "reviewMutationRpc ( deviceId )"
  ],
  "core/taskReviewLifecycle.js": [
    "repository . mutate ( verb )"
  ],
  // taskReviewReads.js:74,151 create adapters for tasks.review.diff at :102,140.
  "core/taskReviewReads.js": [
    "reviewMutationRpc ( deviceId )",
    "reviewMutationRpc ( scope . deviceId )"
  ],
  "core/taskReviewSupport.js": [
    "callRpc ( ... asked )",
    "callRpc ( ... asked )",
    "context . rpc ( ... asked )",
    "reviewRpc ( context )"
  ],
  // taskSummaryReader.js:68,79 call the local pagination helper; :28 sends tasks.list.
  "core/taskSummaryReader.js": [
    "ask ( next )",
    "ask ( { ... next , contentRead , cursor : held . next_cursor } )"
  ],
  "core/taskWaitingOnActions.js": [
    "call ( method )"
  ],
  "core/trackerPages.js": [
    "ask ( asked )"
  ],
  // trackerTaskPage.js:122 forwards task actions/read_through; :147 watches/unwatches; :920 updates tasks.
  "core/trackerTaskPage.js": [
    "authority . callRpc ( method )",
    "authority . callRpc ( method )",
    "owner . callRpc ( method )",
    "state . callRpc ( ... args )",
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
  assert.ok(normalized.includes('forceRefresh = async ( kinds = [ "status" , "log" ] , checkOnly = false , active = alwaysActive ) =>'), "Review the new default git kind expansion");
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

test("secrets metadata routes are explicit and sealed uploads are bounded", async () => {
  const manifest = JSON.parse(await readFile(new URL("fixtures/api/http/resource-manifest.json", root), "utf8"));
  for (const [method, path] of [
    ["secrets.create", "/v1/secrets"],
    ["secrets.list", "/v1/secret-queries"],
    ["secrets.revoke", "/v1/secrets/{secret_id}/revocations"],
    ["secrets.audit", "/v1/secrets/{secret_id}/audit-queries"],
  ]) {
    const operation = manifest.operations.find((entry) => entry.method === method);
    assert.equal(operation?.verb, "POST", method);
    assert.equal(operation?.path, path, method);
    assert.equal(operation?.requires_events, false, method);
    if (method === "secrets.create") assert.equal(operation.requestMaxBytes, 65536);
  }
  assert.ok(!manifest.operations.some(({ method }) => method === "secrets.get"), "Value release has no public HTTP route");
});

test("the generator inventories ordinary, required-context and optional-context methods", () => {
  const output = execFileSync(process.execPath, [fileURLToPath(new URL("scripts/http-resource-manifest.mjs", root)), "--check"], { encoding: "utf8" });
  assert.match(output, /157 explicit resource routes; generated artifacts current/);
});
