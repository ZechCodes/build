/** @vitest-environment jsdom */
// An issue's attachments on its own page (#116): pictures and videos on the
// body and on every comment are thumbnails that open the one lightbox, which
// steps through the list that was pressed, plays a video with its controls,
// and hands focus back to the thumbnail when it closes. The bytes are painted
// from the cache.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { columns, comment, event, issue } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({
    changes: { subscriptions: true, kinds: ["issues"] },
    issues: { attachments: true, watching: false },
  }),
  watchChanges: () => ({ dispose: () => {} }),
}));
vi.mock("../src/core/notify.js", () => ({ notifyError: vi.fn() }));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/trackerAssigneePicker.js", () => ({
  openAssigneePicker: () => ({ close: () => {}, setCatalog: () => {} }),
}));

const SHOT = { name: "before.png", path: "/store/aa-before.png", mime: "image/png", size: 3 };
const CLIP = { name: "drag.mp4", path: "/store/bb-drag.mp4", mime: "video/mp4", size: 3 };
const LOG = { name: "vitest.log", path: "/store/cc-vitest.log", mime: "text/plain", size: 5 };
const AFTER = { name: "after.png", path: "/store/dd-after.png", mime: "image/png", size: 3 };
const WEBM = { name: "rail.webm", path: "/store/ee-rail.webm", mime: "video/webm", size: 3 };

const BYTES = {
  [SHOT.path]: "AAAA",
  [CLIP.path]: "BBBB",
  [LOG.path]: "Q0NDQ0M=",
  [AFTER.path]: "DDDD",
  [WEBM.path]: "EEEE",
};

let host, call, page, mountIssuePage;

const answer = () => ({
  issue: issue({ id: "issue-1", number: 116, attachments: [SHOT, CLIP, LOG] }),
  timeline: [
    event({ id: "ie-1" }),
    comment({ id: "ic-1", body: "After the fix.", attachments: [AFTER, WEBM] }),
  ],
});

const attachmentAnswer = (params) => {
  const known = [SHOT, CLIP, LOG, AFTER, WEBM].find((one) => one.path === params.path);
  return { path: params.path, size: known.size, mime: known.mime, offset: 0, content_b64: BYTES[params.path] };
};

const mount = () => {
  page = mountIssuePage(host, {
    projectId: "proj-1",
    deviceId: "dev-1",
    projectKey: "dev-1|proj-1",
    issueId: "issue-1",
    callRpc: call,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ workspaces: [], items: [] }),
    navigate: vi.fn(),
  });
  return page;
};

const bodyList = () => host.querySelector(".issue-page-attachments");
const commentList = () => host.querySelector(".issue-comment-attachments");
const lightbox = () => document.querySelector(".thread-lightbox");
const press = (key, target = document) =>
  target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));

const openedOn = async (preview) => {
  preview.focus();
  preview.click();
  await vi.waitFor(() => expect(lightbox()?.querySelector(".thread-lightbox-stage img, .thread-lightbox-stage video")).toBeTruthy());
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="pane"></div>';
  host = document.querySelector("#pane");
  const trackerCache = await import("../src/core/trackerCache.js");
  ({ mountIssuePage } = await import("../src/core/trackerIssuePage.js"));
  await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [], columns: columns() });
  call = vi.fn(async (method, params) => {
    if (method === "issues.get") return answer();
    if (method === "issues.attachment") return attachmentAnswer(params);
    return {};
  });
});

afterEach(() => {
  page?.dispose();
  document.querySelectorAll(".modal-scrim").forEach((scrim) => scrim.remove());
});

describe("attachment thumbnails on an issue", () => {
  it("draws the body's and each comment's pictures and videos as thumbnails, and other files as download chips", async () => {
    mount();
    await vi.waitFor(() => expect(commentList()).toBeTruthy());

    expect(bodyList().querySelector(`img.thread-attachment-image[data-attachment-path="${SHOT.path}"]`)).toBeTruthy();
    const clip = bodyList().querySelector(`video.thread-attachment-video[data-attachment-path="${CLIP.path}"]`);
    expect(clip).toBeTruthy();
    expect(clip.hasAttribute("controls")).toBe(false);
    expect(clip.closest("button").getAttribute("aria-label")).toBe("Play drag.mp4");
    expect(bodyList().querySelector(".thread-attachment-play svg")).toBeTruthy();
    const chip = bodyList().querySelector("button.thread-attachment");
    expect(chip.dataset.attachmentPath).toBe(LOG.path);
    expect(chip.getAttribute("title")).toBe("Download vitest.log");

    expect(commentList().closest(".issue-comment-card")).toBeTruthy();
    expect(commentList().querySelectorAll("button.thread-attachment-preview")).toHaveLength(2);

    // The thumbnails fill from the bytes, the frame of a short video included.
    await vi.waitFor(() => {
      expect(bodyList().querySelector("img.thread-attachment-image").getAttribute("src")).toBe("data:image/png;base64,AAAA");
      expect(clip.getAttribute("src")).toBe("data:video/mp4;base64,BBBB");
    });
  });
});

describe("the lightbox on an issue", () => {
  it("opens a picture, steps through that list only with the arrow keys, and closes on Escape with focus on the thumbnail showing", async () => {
    mount();
    await vi.waitFor(() => expect(commentList()).toBeTruthy());
    const [shot, clip] = bodyList().querySelectorAll("button.thread-attachment-preview");

    await openedOn(shot);
    expect(lightbox().querySelector(".thread-lightbox-stage img").getAttribute("src")).toBe("data:image/png;base64,AAAA");
    expect(lightbox().querySelector(".thread-lightbox-count").textContent).toBe("1 of 2");
    expect(lightbox().getAttribute("aria-label")).toBe("Image preview: before.png");

    press("ArrowRight");
    await vi.waitFor(() => expect(lightbox().querySelector(".thread-lightbox-stage video")).toBeTruthy());
    expect(lightbox().querySelector(".thread-lightbox-count").textContent).toBe("2 of 2");

    // The body's list is two long: the comment's files are not in it, and
    // the log is a download, not a slide.
    press("ArrowRight");
    await vi.waitFor(() => expect(lightbox().querySelector(".thread-lightbox-count").textContent).toBe("1 of 2"));
    press("ArrowLeft");
    await vi.waitFor(() => expect(lightbox().querySelector(".thread-lightbox-stage video")).toBeTruthy());

    press("Escape");
    await vi.waitFor(() => expect(lightbox()).toBeNull());
    expect(document.activeElement).toBe(clip);
  });

  it("plays a video inline with its controls", async () => {
    mount();
    await vi.waitFor(() => expect(commentList()).toBeTruthy());
    const webm = commentList().querySelectorAll("button.thread-attachment-preview")[1];

    await openedOn(webm);
    const video = lightbox().querySelector(".thread-lightbox-stage video");
    expect(video.hasAttribute("controls")).toBe(true);
    expect(video.hasAttribute("playsinline")).toBe(true);
    expect(video.getAttribute("src")).toBe("data:video/webm;base64,EEEE");
    expect(lightbox().getAttribute("aria-label")).toBe("Video: rail.webm");

    // The arrows belong to the video while it has focus: they seek, not step.
    video.focus();
    press("ArrowLeft", video);
    expect(lightbox().querySelector(".thread-lightbox-count").textContent).toBe("2 of 2");
  });

  it("closes on a press on the backdrop, and focus goes back to the thumbnail", async () => {
    mount();
    await vi.waitFor(() => expect(commentList()).toBeTruthy());
    const after = commentList().querySelector("button.thread-attachment-preview");

    await openedOn(after);
    const scrim = document.querySelector(".modal-scrim");
    scrim.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await vi.waitFor(() => expect(lightbox()).toBeNull());
    expect(document.activeElement).toBe(after);
  });

  it("steps on a sideways swipe and ignores a vertical one", async () => {
    mount();
    await vi.waitFor(() => expect(commentList()).toBeTruthy());
    await openedOn(commentList().querySelector("button.thread-attachment-preview"));
    const stage = lightbox().querySelector(".thread-lightbox-stage");
    const touch = (type, x, y) => {
      const touchEvent = new Event(type, { bubbles: true });
      Object.defineProperty(touchEvent, "changedTouches", { value: [{ clientX: x, clientY: y }] });
      stage.dispatchEvent(touchEvent);
    };

    touch("touchstart", 300, 200);
    touch("touchend", 290, 40);
    expect(lightbox().querySelector(".thread-lightbox-count").textContent).toBe("1 of 2");

    touch("touchstart", 300, 200);
    touch("touchend", 120, 210);
    await vi.waitFor(() => expect(lightbox().querySelector(".thread-lightbox-count").textContent).toBe("2 of 2"));
    await vi.waitFor(() => expect(lightbox().querySelector(".thread-lightbox-stage video")).toBeTruthy());
  });

  it("keeps Tab inside the lightbox", async () => {
    mount();
    await vi.waitFor(() => expect(commentList()).toBeTruthy());
    await openedOn(bodyList().querySelector("button.thread-attachment-preview"));
    const stops = [...lightbox().querySelectorAll("button")];
    stops.at(-1).focus();
    press("Tab");
    expect(document.activeElement).toBe(stops[0]);
    press("Tab");
    expect(document.activeElement).toBe(stops[1]);
  });

  it("says so in the lightbox when the bytes are refused", async () => {
    call = vi.fn(async (method) => {
      if (method === "issues.get") return answer();
      if (method === "issues.attachment") throw new Error("not an attachment on this issue");
      return {};
    });
    mount();
    await vi.waitFor(() => expect(commentList()).toBeTruthy());
    const clip = bodyList().querySelectorAll("button.thread-attachment-preview")[1];
    await vi.waitFor(() => expect(clip.closest("figure").classList.contains("unavailable")).toBe(true));
    clip.click();
    await vi.waitFor(() => expect(lightbox()?.querySelector(".thread-lightbox-note")?.textContent)
      .toBe("This attachment could not be loaded."));
  });
});

describe("attachment bytes and the cache", () => {
  it("paints a revisit's thumbnails from the cache without asking the bridge", async () => {
    mount();
    await vi.waitFor(() =>
      expect(bodyList()?.querySelector("img.thread-attachment-image")?.getAttribute("src")).toBe("data:image/png;base64,AAAA"));
    page.dispose();
    page = null;

    call = vi.fn(async (method) => (method === "issues.get" ? answer() : new Promise(() => {})));
    mount();
    await vi.waitFor(() =>
      expect(bodyList()?.querySelector("img.thread-attachment-image")?.getAttribute("src")).toBe("data:image/png;base64,AAAA"));
    expect(call.mock.calls.filter(([method]) => method === "issues.attachment")).toHaveLength(0);
  });

  it("still draws the thumbnails and the lightbox when the cache refuses to keep them", async () => {
    // The page paints; persisting an attachment body is what fails, and the
    // cache stands down for the session the way a full or locked-down
    // IndexedDB makes it.
    const put = IDBObjectStore.prototype.put;
    const refusing = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (value, key) {
      if (String(key).includes("|attachment|")) throw new DOMException("quota", "QuotaExceededError");
      return put.call(this, value, key);
    });
    mount();
    await vi.waitFor(() =>
      expect(bodyList()?.querySelector("img.thread-attachment-image")?.getAttribute("src")).toBe("data:image/png;base64,AAAA"));
    expect(bodyList().querySelector(".thread-attachment-figure.unavailable")).toBeNull();
    await openedOn(commentList().querySelectorAll("button.thread-attachment-preview")[1]);
    expect(lightbox().querySelector(".thread-lightbox-stage video").getAttribute("src")).toBe("data:video/webm;base64,EEEE");
    expect(refusing).toHaveBeenCalled();
    refusing.mockRestore();
  });
});
