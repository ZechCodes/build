// @vitest-environment jsdom
// Which build this tab is running, in Settings.
//
// The bundle has always carried its sha — core/version.js reads it to decide
// whether the tab is stale — but nothing ever showed it, so on the night it
// mattered there was no way to answer "which build is this". Short on the page
// for reading, whole in the title and on the clipboard for pasting.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  buildVersionLineHtml,
  fullBuildVersion,
  mountBuildVersionLine,
  shortBuildVersion,
} from "../src/core/buildVersionLine.js";

const SHA = "631836cc1f4b9e2a7d05c8e3b1a6f40d9c2e5b78";

const mount = (version, over = {}) => {
  document.body.innerHTML = `<div id="host">${buildVersionLineHtml(version)}</div>`;
  mountBuildVersionLine(document.querySelector("#host"), { version, clipboard: null, ...over });
};

const sha = () => document.querySelector("#buildversionsha");
const copy = () => document.querySelector("#buildversioncopy");

afterEach(() => {
  document.body.innerHTML = "";
});

describe("how a version reads", () => {
  it("is git's own abbreviation of a sha", () => {
    expect(shortBuildVersion(SHA)).toBe("631836c");
  });

  it("leaves anything that is not a sha whole, rather than cutting a word", () => {
    expect(shortBuildVersion("dev")).toBe("dev");
    expect(shortBuildVersion("v2.1.0")).toBe("v2.1.0");
    expect(shortBuildVersion("release-candidate")).toBe("release-candidate");
  });

  it("says dev for a bundle nothing stamped", () => {
    expect(shortBuildVersion("")).toBe("dev");
    expect(shortBuildVersion(undefined)).toBe("dev");
    expect(fullBuildVersion("")).toBe("dev");
    expect(fullBuildVersion(null)).toBe("dev");
  });

  it("keeps the whole sha for pasting", () => {
    expect(fullBuildVersion(SHA)).toBe(SHA);
    expect(fullBuildVersion(`  ${SHA}  `)).toBe(SHA);
  });
});

describe("the line on the page", () => {
  it("reads Build and the short sha, with the whole one in the title", () => {
    mount(SHA);

    expect(document.querySelector("#buildversion").textContent.replace(/\s+/g, " ").trim())
      .toBe("Build 631836c Copy");
    expect(sha().textContent).toBe("631836c");
    expect(sha().getAttribute("title")).toBe(SHA);
    expect(sha().classList.contains("mono")).toBe(true);
  });

  it("shows dev on a bundle CI never stamped", () => {
    mount("dev");

    expect(sha().textContent).toBe("dev");
    expect(sha().getAttribute("title")).toBe("dev");
  });

  it("escapes a version it did not write", () => {
    mount('"><script>alert(1)</script>');

    expect(document.querySelector("script")).toBeNull();
    expect(sha().textContent).toBe('"><script>alert(1)</script>');
  });
});

describe("copying it", () => {
  it("puts the WHOLE sha on the clipboard, not the short form", async () => {
    const writeText = vi.fn(async () => {});
    mount(SHA, { clipboard: { writeText } });

    copy().click();
    await vi.waitFor(() => expect(copy().textContent).toBe("Copied"));

    expect(writeText).toHaveBeenCalledWith(SHA);
  });

  it("goes through the same textarea fallback the Diagnostics dump uses", async () => {
    document.execCommand = vi.fn(() => true);
    mount(SHA, { clipboard: null });

    copy().click();
    await vi.waitFor(() => expect(document.execCommand).toHaveBeenCalledWith("copy"));

    expect(document.querySelector("textarea")).toBeNull();
    delete document.execCommand;
  });

  it("says so rather than claiming it copied when neither route works", async () => {
    document.execCommand = vi.fn(() => false);
    mount(SHA, { clipboard: { writeText: async () => { throw new Error("denied"); } } });

    copy().click();
    await vi.waitFor(() => expect(copy().textContent).toBe("Press and hold to copy"));

    delete document.execCommand;
  });

  it("copies dev too, so the answer is never silence", async () => {
    const writeText = vi.fn(async () => {});
    mount("", { clipboard: { writeText } });

    copy().click();
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("dev"));
  });

  it("wires nothing, and throws nothing, on a host with no line in it", () => {
    document.body.innerHTML = '<div id="host"></div>';

    expect(() => mountBuildVersionLine(document.querySelector("#host"), { version: SHA })).not.toThrow();
  });
});
