// The capture decision page's pure model: what the page has to say about one
// capture, the router's offer read as choices a user can tap, and the call each
// way of answering makes.

import { describe, it, expect } from "vitest";
import {
  answerParams,
  captureCancelConfirm,
  captureDecisionHtml,
  captureDecisionModel,
  manualRouteAnswer,
  manualRouteParams,
  optionDestinationText,
} from "../src/core/captureDecision.js";

const projects = [
  { id: "p1", name: "relaydb" },
  { id: "p2", name: "dotfiles" },
];

/** A capture record as `capture.get` answers with one. */
const capture = (over = {}) => ({
  id: "capture-1",
  text: "fix the login redirect",
  created_at: "2026-08-15T10:00:00Z",
  state: "unrouted",
  routing: null,
  question: null,
  ...over,
});

const question = (over = {}) => ({
  text: "Which project is the login redirect in?",
  asked_at: "2026-08-15T10:00:01Z",
  answer: null,
  options: [],
  chosen_option_id: null,
  ...over,
});

const option = (over = {}) => ({
  id: "option-1",
  label: "File as an issue on relaydb",
  project_id: "p1",
  kind: "issue",
  branch: null,
  ...over,
});

describe("the capture decision model", () => {
  it("says what was captured and what is happening to it", () => {
    const model = captureDecisionModel(capture({ state: "routing" }), { projects });
    expect(model.captureId).toBe("capture-1");
    expect(model.said).toBe("fix the login redirect");
    expect(model.statusText).toBe("Deciding where this goes");
    expect(model.spinning).toBe(true);
    expect(model.awaitingAnswer).toBe(false);
    expect(model.options).toEqual([]);
  });

  it("stops spinning and names the user as the one holding it up", () => {
    const model = captureDecisionModel(capture({ question: question() }), { projects });
    expect(model.question).toBe("Which project is the login redirect in?");
    expect(model.awaitingAnswer).toBe(true);
    expect(model.statusText).toBe("Waiting for your answer");
    expect(model.spinning).toBe(false);
  });

  it("reads the router's offer as choices, in the order it numbered them", () => {
    const model = captureDecisionModel(
      capture({
        question: question({
          options: [
            option(),
            option({ id: "option-2", label: "Continue the login work", kind: "branch", branch: "build/login" }),
            option({ id: "option-3", label: "It is about dotfiles", project_id: "p2", kind: null }),
          ],
        }),
      }),
      { projects },
    );
    expect(model.options.map((choice) => choice.id)).toEqual(["option-1", "option-2", "option-3"]);
    expect(model.options.map((choice) => choice.destination)).toEqual([
      "relaydb · a new issue",
      "relaydb · branch build/login",
      "dotfiles",
    ]);
  });

  it("says where a routed capture went, and that the deciding is over", () => {
    const model = captureDecisionModel(
      capture({ state: "routed", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }),
      { projects },
    );
    expect(model.routed).toBe(true);
    expect(model.statusText).toBe("→ relaydb as issue");
    expect(model.spinning).toBe(false);
  });

  it("carries the answer already given, and which choice it was", () => {
    const model = captureDecisionModel(
      capture({
        state: "routing",
        question: question({ answer: "File as an issue on relaydb", chosen_option_id: "option-1", options: [option()] }),
      }),
      { projects },
    );
    expect(model.awaitingAnswer).toBe(false);
    expect(model.answer).toBe("File as an issue on relaydb");
    expect(model.chosenOptionId).toBe("option-1");
    expect(model.statusText).toBe("Deciding where this goes");
  });

  it("reads a capture that has not landed yet without inventing one", () => {
    const model = captureDecisionModel(null, { projects });
    expect(model.said).toBe("");
    expect(model.options).toEqual([]);
    expect(model.awaitingAnswer).toBe(false);
  });

  it("names a project the feed has never heard of by its id", () => {
    const model = captureDecisionModel(capture({ question: question({ options: [option({ project_id: "p9" })] }) }), {
      projects,
    });
    expect(model.options[0].destination).toBe("p9 · a new issue");
  });
});

describe("what an option stands for", () => {
  it("spells out the destination the router named", () => {
    expect(optionDestinationText(option(), projects)).toBe("relaydb · a new issue");
    expect(optionDestinationText(option({ kind: "branch" }), projects)).toBe("relaydb · a new branch");
    expect(optionDestinationText(option({ kind: "branch", branch: "build/login" }), projects)).toBe(
      "relaydb · branch build/login",
    );
    // A branch name says it is a branch, whatever the kind field says.
    expect(optionDestinationText(option({ kind: null, branch: "build/login" }), projects)).toBe(
      "relaydb · branch build/login",
    );
  });

  it("says nothing at all about an option that is only a label", () => {
    expect(optionDestinationText(option({ project_id: null, kind: null }), projects)).toBe("");
  });
});

describe("routing it by hand", () => {
  it("answers the router in the terms it routes in", () => {
    expect(manualRouteAnswer({ projectId: "p1", kind: "issue" })).toBe("Route this to project p1 as an issue");
    expect(manualRouteAnswer({ projectId: "p1", kind: "branch" })).toBe("Route this to project p1 as a branch");
    expect(manualRouteAnswer({ projectId: "p1", kind: "branch", branch: "build/login" })).toBe(
      "Route this to project p1 as a branch, on the branch build/login",
    );
  });

  it("names no destination without a project, because there is none", () => {
    expect(manualRouteAnswer({ projectId: "", kind: "issue" })).toBe("");
  });

  it("asks the daemon for the same destination when there is no question to answer", () => {
    expect(manualRouteParams("capture-1", { projectId: "p2", kind: "issue" })).toEqual({
      capture_id: "capture-1",
      project_id: "p2",
      kind: "issue",
    });
    expect(manualRouteParams("capture-1", { projectId: "p2", kind: "branch", branch: " build/login " })).toEqual({
      capture_id: "capture-1",
      project_id: "p2",
      kind: "branch",
      branch: "build/login",
    });
    // An unnamed branch is the daemon naming it after what was said.
    expect(manualRouteParams("capture-1", { projectId: "p2", kind: "branch", branch: "  " })).toEqual({
      capture_id: "capture-1",
      project_id: "p2",
      kind: "branch",
    });
  });
});

describe("what an answer asks for", () => {
  it("names the option that was tapped", () => {
    expect(answerParams("capture-1", { optionId: "option-2" })).toEqual({
      capture_id: "capture-1",
      option_id: "option-2",
    });
  });

  it("sends the words that were typed, trimmed", () => {
    expect(answerParams("capture-1", { text: "  it is the relay  " })).toEqual({
      capture_id: "capture-1",
      text: "it is the relay",
    });
  });

  it("asks for nothing when nothing was said", () => {
    expect(answerParams("capture-1", { text: "   " })).toBeNull();
    expect(answerParams("capture-1", {})).toBeNull();
  });
});

describe("cancelling a capture", () => {
  it("outlines what goes, as the destructive verb it is", () => {
    const confirmation = captureCancelConfirm(captureDecisionModel(capture(), { projects }));
    expect(confirmation.danger).toBe(true);
    expect(confirmation.actions.join(" ")).toContain("router");
    expect(confirmation.actions.join(" ").toLowerCase()).toContain("forget");
    expect(confirmation.intro).toContain("fix the login redirect");
  });
});

describe("the decision page's markup", () => {
  const model = () =>
    captureDecisionModel(
      capture({ question: question({ options: [option(), option({ id: "option-2", label: "New branch" })] }) }),
      { projects },
    );

  it("offers every choice as its own control, and always the way out", () => {
    const html = captureDecisionHtml(model(), { projects, projectId: "p1", kind: "issue", branch: "", answer: "" });
    expect(html).toContain('data-capture-option="option-1"');
    expect(html).toContain('data-capture-option="option-2"');
    expect(html).toContain('id="capture-answer"');
    expect(html).toContain('id="capture-cancel"');
    expect(html).toContain('id="capture-project"');
  });

  it("discloses the branch field only for a branch", () => {
    const ui = { projects, projectId: "p1", branch: "", answer: "", branches: ["build/login"] };
    expect(captureDecisionHtml(model(), { ...ui, kind: "issue" })).not.toContain('id="capture-branch"');
    const branchy = captureDecisionHtml(model(), { ...ui, kind: "branch" });
    expect(branchy).toContain('id="capture-branch"');
    expect(branchy).toContain('<option value="build/login">');
  });

  it("escapes what the user said, what the router asked, and what it offered", () => {
    const hostile = captureDecisionModel(
      capture({
        text: '<img src=x onerror="alert(1)">',
        question: question({ text: "<script>alert(2)</script>", options: [option({ label: "<b>tap me</b>" })] }),
      }),
      { projects },
    );
    const html = captureDecisionHtml(hostile, { projects, projectId: "p1", kind: "issue", branch: "", answer: "" });
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<b>tap me</b>");
  });
});
