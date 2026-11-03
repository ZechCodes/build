// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeCache } from "../src/core/localCache.js";
import { rememberReviewSupport, readReviewSupport, canReviewOperation } from "../src/core/taskReviewSupport.js";
import { capabilitiesOf } from "../src/core/bridgeApi/v1/index.js";
import { writeReviewRecord } from "../src/core/taskReviewCache.js";
import { reviewHeadHtml } from "../src/core/taskReviewRender.js";
import { mountTaskReviewPage } from "../src/core/taskReviewPage.js";

const scope = { deviceId: "loop", projectId: "p", taskId: "t" };
const revisions = [1, 2].map((number) => ({ id: `r${number}`, number, directories: [] }));
const saved = { mode: "pull_request", version: 1, snapshots: revisions,
  pull_request: { status: "open", author: "author" }, verdict_deliveries: [] };
const task = { id: "t", assignee: { kind: "agent", agent_id: "reviewer" },
  identities: { author: { name: "Ada" }, reviewer: { name: "Grace" } } };
const support = { get: true, pullRequests: true, author: true, reject: true, verdictDelivery: true, retryDelivery: true, since: true };
let page;
beforeEach(async () => {
  page?.dispose();
  await wipeCache();
  document.body.innerHTML = '<div id="root"></div>';
});
it("retains independently negotiated loop capabilities in cache and gates retry",async () => {
  const flags = capabilitiesOf({api_version:"3.30.0",capabilities:["tasks.review.author","tasks.review.reject","tasks.review.verdictDelivery","tasks.review.retryDelivery"]}).reviews;
  await rememberReviewSupport("loop",{reviews:flags});
  expect(await readReviewSupport("loop")).toMatchObject({author:true,reject:true,verdictDelivery:true,retryDelivery:true});
  expect(canReviewOperation({...support,verdictDelivery:false},"retryDelivery",saved)).toBe(false);
  expect(canReviewOperation(support,"retryDelivery",saved)).toBe(true);
});
it.each([["delivered","Delivered"],["pending","Pending (agent unavailable)"],["failed","Failed"]])("paints cached %s delivery with author and deciding reviewer", (state, label) => {
  const review = {...saved,verdict_deliveries:[{comment_id:"c",state}]};
  const html = reviewHeadHtml(review,revisions[1],support, { task, scope });
  expect(html).toContain("by @Ada"); expect(html).toContain("@Grace"); expect(html).toContain(label);
  expect(html.includes("data-review-retry-delivery")).toBe(state==="failed");
  const older = reviewHeadHtml(review,revisions[1],{},{task,scope});
  expect(older).not.toContain("by @Ada"); expect(older).not.toContain(label);
});
it("uses journal order for the latest verdict and sends only its failed comment identity on retry",async () => {
  await rememberReviewSupport("loop",{reviews:support});
  const review = {...saved,verdict_deliveries:[{comment_id:"old",state:"failed"},{comment_id:"latest",state:"failed"}]};
  await writeReviewRecord(scope,review,1);
  const callRpc = vi.fn(async(method)=>method==="tasks.review.retryDelivery"?{review:{...review,version:2,verdict_deliveries:[{comment_id:"latest",state:"pending"}]}}:new Promise(() => {}));
  page = mountTaskReviewPage(document.querySelector("#root"),{...scope,task:() => task,callRpc}); await page.ready;
  document.querySelector("[data-review-retry-delivery]").click();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("tasks.review.retryDelivery",{task_id:"t",comment_id:"latest"}));
  await vi.waitFor(() => expect(document.querySelector("[data-review-delivery]").textContent).toContain("Pending"));
});
it("opens the newer revision since the last user verdict while offline",async () => {
  await rememberReviewSupport("loop",{reviews:support}); await writeReviewRecord(scope,saved,1);
  const navigate = vi.fn();
  page = mountTaskReviewPage(document.querySelector("#root"),{...scope,task:() => task,callRpc:() => new Promise(() => {}),markReviewRoute:navigate,reviewRoute:{revision:1,mode:"all"},rows:() => [{actor:{kind:"user"},opinion:{snapshot_id:"r1",verdict:"request_changes"}}]}); await page.ready;
  expect(document.querySelector("[data-review-newer-snapshot]").textContent).toContain("Revision 2 is ready");
  document.querySelector("[data-review-new-revision]").click();
  expect(navigate.mock.calls.at(-1)[0].review).toMatchObject({revision:2,mode:"since"});
  expect(document.querySelector('[data-review-range="since"]').getAttribute("aria-pressed")).toBe("true");
});

it("gates Rejected and Legacy header chips independently of cached metadata", () => {
  const rejected = { ...saved, pull_request: { status: "rejected" } };
  expect(reviewHeadHtml(rejected, revisions[1], support)).toContain("Rejected");
  expect(reviewHeadHtml(rejected, revisions[1], {})).not.toContain("Rejected");
  const legacy = { ...saved, mode: "snapshot" };
  expect(reviewHeadHtml(legacy, revisions[1], support)).toContain("Legacy review");
  expect(reviewHeadHtml(legacy, revisions[1], {})).not.toContain("Legacy review");
});
