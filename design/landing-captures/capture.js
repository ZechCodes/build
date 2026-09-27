// One landing screen: the scene named in the query, in the app's own frame,
// inside the device's system chrome. capture.mjs drives it.
// The views' own sheets, after styles.css as the app loads them.
import "../../spa/src/styles/shell.css";
import "../../spa/src/styles/surfaces.css";
import "../../spa/src/styles/tasks.css";
import "../../spa/src/styles/fileEditor.css";
import { renderSystemShell } from "./system-ui.js";
import { paintBubbles } from "./app-shell.js";
import { conversationScene, editorScene, gitScene, mergedScene, triageScene } from "./workspace-scenes.js";
import { boardScene, builderScene, teamScene } from "./project-scenes.js";

const query = new URLSearchParams(location.search);
const scene = query.get("scene") || "ui10-editor";
const state = query.get("state") || "default";
const profile = query.get("profile") || "app";

// The review sequence is the triage on the tablet, where act 7 happens, and
// the merged workspace everywhere else.
const reviewScene = () => (profile === "ipad" ? triageScene(state) : mergedScene());

const scenes = {
  ui03: () => conversationScene(state),
  ui05: reviewScene,
  "ui10-editor": editorScene,
  "ui12-tasks": boardScene,
  "ui13-team": teamScene,
  "ui14-git": gitScene,
  "ui15-triage": () => triageScene("triage"),
  "ui16-builder": builderScene,
};

// A phone keeps the inbox away behind its toggle, as the app does at that width.
document.body.classList.toggle("inbox-collapsed", profile === "iphone");
const capture = document.querySelector("#capture");
capture.innerHTML = renderSystemShell(profile, (scenes[scene] || editorScene)());
paintBubbles(capture);
