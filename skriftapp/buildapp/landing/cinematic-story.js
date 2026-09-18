import {
  STORY_SCENES,
  clamp,
  frameAtTravel,
  profileForViewport,
  storyTravel,
  travelAtFrame,
} from "./story-manifest.js";

const ENHANCED_CLASS = "is-story-enhanced";
const ACTIVE_CLASS = "is-active";
const FRAME_EVENT = "build:storyframe";
const READABLE_LOCAL = 0.42;

function canEnhance(profile, reducedMotion, saveData) {
  return profile !== "static" && !reducedMotion && !saveData;
}

function storyTop(story) {
  return story.getBoundingClientRect().top + window.scrollY;
}

function stageHeight(stage) {
  return Math.max(1, stage.getBoundingClientRect().height);
}

function readerPosition(story, stage) {
  const top = storyTop(story);
  const end = top + story.offsetHeight;
  const pinnedEnd = end - stageHeight(stage);
  return {
    inPinnedStory: window.scrollY >= top - 1 && window.scrollY <= pinnedEnd + 1,
    afterPinnedStory: window.scrollY > pinnedEnd,
    offsetFromEnd: window.scrollY - end,
  };
}

const LEAVE_START = 0.7;
const ENTER_START = 0.85;

// The outgoing scene is fully gone before the incoming one starts, so two
// headlines never share the stage.
function opacityForScene(index, frame) {
  if (index === frame.sceneIndex) {
    if (index === STORY_SCENES.length - 1 || frame.local <= LEAVE_START) return 1;
    return Math.max(0, 1 - (frame.local - LEAVE_START) / (ENTER_START - LEAVE_START));
  }
  if (index === frame.sceneIndex + 1 && frame.local > ENTER_START) {
    return (frame.local - ENTER_START) / (1 - ENTER_START);
  }
  return 0;
}

// How far the next scene has settled in: its copy slides up as it fades in.
function enterForScene(index, frame) {
  if (index === frame.sceneIndex + 1) return clamp(opacityForScene(index, frame));
  return 1;
}

function dispatchFrame(stage, frame) {
  stage.dispatchEvent(new CustomEvent(FRAME_EVENT, { detail: { ...frame, stage } }));
}

function setSceneInteractive(scene, interactive) {
  const controls = scene.querySelectorAll("a, button, input, select, textarea, [tabindex]");
  controls.forEach((control) => {
    if (!interactive) {
      if (!control.hasAttribute("data-story-tabindex")) {
        control.dataset.storyTabindex = control.getAttribute("tabindex") ?? "";
      }
      control.setAttribute("tabindex", "-1");
      return;
    }
    if (!control.hasAttribute("data-story-tabindex")) return;
    const previous = control.dataset.storyTabindex;
    if (previous) control.setAttribute("tabindex", previous);
    else control.removeAttribute("tabindex");
    delete control.dataset.storyTabindex;
  });
}

// The first scene starts under the fixed nav, so the document version of it is
// the top of the page; later scenes rely on their scroll margin.
function revealScene(scenes, sceneIndex) {
  if (sceneIndex === 0) {
    window.scrollTo({ top: 0, behavior: "instant" });
    return;
  }
  scenes[sceneIndex]?.scrollIntoView({ block: "start" });
}

function applyFrame({ frame, stage, scenes, progressElement, positionElement }) {
  stage.dataset.scene = frame.scene;
  stage.dataset.profile = frame.profile;
  stage.dataset.checkpoint = frame.checkpoint;
  stage.style.setProperty("--story-progress", String(frame.progress));
  stage.style.setProperty("--scene-local", String(frame.local));

  const activeChanged = stage.dataset.sceneIndex !== String(frame.sceneIndex);
  stage.dataset.sceneIndex = String(frame.sceneIndex);
  scenes.forEach((scene, index) => {
    const isActive = index === frame.sceneIndex;
    scene.style.setProperty("--scene-opacity", String(clamp(opacityForScene(index, frame))));
    scene.style.setProperty("--scene-enter", String(enterForScene(index, frame)));
    if (activeChanged) {
      scene.classList.toggle(ACTIVE_CLASS, isActive);
      setSceneInteractive(scene, isActive);
    }
    scene.dataset.checkpoint = isActive ? frame.checkpoint : "";
  });

  if (progressElement) progressElement.style.transform = `scaleX(${frame.progress})`;
  if (positionElement?.dataset.sceneIndex !== String(frame.sceneIndex)) {
    positionElement.dataset.sceneIndex = String(frame.sceneIndex);
    positionElement.textContent = `Scene ${frame.sceneIndex + 1} of ${STORY_SCENES.length}: ${STORY_SCENES[frame.sceneIndex].copy.title}`;
  }
  dispatchFrame(stage, frame);
}

function clearEnhancedState({ stage, scenes }) {
  stage.removeAttribute("data-scene");
  stage.removeAttribute("data-checkpoint");
  stage.removeAttribute("data-profile");
  stage.removeAttribute("data-scene-index");
  stage.style.removeProperty("--story-progress");
  stage.style.removeProperty("--scene-local");
  scenes.forEach((scene) => {
    scene.style.removeProperty("--scene-opacity");
    scene.style.removeProperty("--scene-enter");
    scene.classList.remove(ACTIVE_CLASS);
    setSceneInteractive(scene, true);
    scene.dataset.checkpoint = "";
  });
}

export function installCinematicStory({ story, stage }) {
  const page = story.closest("[data-cinematic-page]");
  const scenes = Array.from(stage.querySelectorAll("[data-story-scene]"));
  const progressElement = stage.querySelector("[data-story-progress]");
  const positionElement = stage.querySelector("[data-story-position]");
  const reducedMotionQuery = matchMedia("(prefers-reduced-motion: reduce)");
  const connection = navigator.connection;
  const subscribers = new Set();
  let frame = frameAtTravel(0, "desktop");
  let profile = profileForViewport(window.innerWidth, window.innerHeight);
  let enhanced = false;
  let scheduled = false;
  let destroyed = false;

  function currentSaveData() {
    return Boolean(connection?.saveData);
  }

  function publish(nextFrame, apply = enhanced) {
    frame = { ...nextFrame, enhanced };
    if (apply) applyFrame({ frame, stage, scenes, progressElement, positionElement });
    else dispatchFrame(stage, frame);
    subscribers.forEach((subscriber) => subscriber(frame));
  }

  function measureFrame() {
    if (!enhanced) return;
    const height = stageHeight(stage);
    const travelPixels = storyTravel(profile) * height;
    const traveled = clamp(window.scrollY - storyTop(story), 0, travelPixels);
    publish(frameAtTravel(traveled / height, profile));
  }

  function requestFrame() {
    if (scheduled || destroyed) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      measureFrame();
    });
  }

  function seek(sceneIndex, local = READABLE_LOCAL, behavior = "auto") {
    const boundedIndex = clamp(sceneIndex, 0, STORY_SCENES.length - 1);
    if (!enhanced) {
      scenes[boundedIndex]?.scrollIntoView({ block: "start", behavior });
      return;
    }
    const travel = travelAtFrame(boundedIndex, local, profile);
    window.scrollTo({ top: storyTop(story) + travel * stageHeight(stage), behavior });
    requestFrame();
  }

  function sceneIndexFromTarget(target) {
    const scene = target instanceof Element ? target.closest("[data-story-scene]") : null;
    return scene ? scenes.indexOf(scene) : -1;
  }

  function seekHash() {
    const id = decodeURIComponent(location.hash.slice(1));
    if (!id) return false;
    const target = document.getElementById(id);
    const sceneIndex = target ? sceneIndexFromTarget(target) : -1;
    if (sceneIndex < 0) {
      target?.scrollIntoView({ block: "start", behavior: "auto" });
      return false;
    }
    seek(sceneIndex, READABLE_LOCAL, "auto");
    return true;
  }

  // Corrections the story makes to its own position are instant: an animated
  // correction fights whatever scroll the reader is in the middle of.
  function preserveAfterStory(offsetFromEnd, defer = false) {
    const restore = () => window.scrollTo({
      top: storyTop(story) + story.offsetHeight + offsetFromEnd,
      behavior: "instant",
    });
    if (defer) requestAnimationFrame(restore);
    else restore();
  }

  function useDocumentFlow(previous, wasEnhanced, reader, preserveFrame) {
    story.style.removeProperty("--story-travel");
    story.style.removeProperty("--story-height");
    clearEnhancedState({ stage, scenes });
    publish({ ...previous, profile: "static" }, false);
    if (preserveFrame && wasEnhanced && reader.inPinnedStory) {
      requestAnimationFrame(() => revealScene(scenes, previous.sceneIndex));
    } else if (preserveFrame && reader.afterPinnedStory) {
      preserveAfterStory(reader.offsetFromEnd, true);
    }
  }

  function useEnhancedStory(previous, reader, preserveFrame) {
    const height = `${(storyTravel(profile) + 1) * stageHeight(stage)}px`;
    // Safari fires resize as its toolbar collapses during a scroll. The stage
    // is sized in svh, so the pinned geometry is unchanged and the page must
    // not be moved out from under the reader's finger.
    const geometryChanged = story.style.getPropertyValue("--story-height") !== height;
    story.style.setProperty("--story-travel", String(storyTravel(profile)));
    story.style.setProperty("--story-height", height);
    if (preserveFrame && geometryChanged && reader.inPinnedStory) {
      const travel = travelAtFrame(previous.sceneIndex, previous.local, profile);
      window.scrollTo({ top: storyTop(story) + travel * stageHeight(stage), behavior: "instant" });
    } else if (preserveFrame && geometryChanged && reader.afterPinnedStory) {
      preserveAfterStory(reader.offsetFromEnd);
    }
    measureFrame();
  }

  function setMode(nextProfile, preserveFrame = false) {
    const previous = frame;
    const wasEnhanced = enhanced;
    const reader = readerPosition(story, stage);
    const shouldEnhance = canEnhance(nextProfile, reducedMotionQuery.matches, currentSaveData());
    profile = nextProfile;
    enhanced = shouldEnhance;
    page?.classList.toggle(ENHANCED_CLASS, enhanced);
    story.classList.toggle(ENHANCED_CLASS, enhanced);
    story.dataset.storyMode = enhanced ? "enhanced" : "static";
    story.dataset.profile = profile;

    if (!enhanced) {
      useDocumentFlow(previous, wasEnhanced, reader, preserveFrame);
      return;
    }
    useEnhancedStory(previous, reader, preserveFrame);
  }

  function refresh({ preserve = true } = {}) {
    setMode(profileForViewport(window.innerWidth, window.innerHeight), preserve);
  }

  function subscribe(subscriber) {
    subscribers.add(subscriber);
    subscriber(frame);
    return () => subscribers.delete(subscriber);
  }

  function onResize() {
    refresh({ preserve: true });
  }

  function onPreferenceChange() {
    refresh({ preserve: true });
  }

  function onVisibilityChange() {
    if (!document.hidden) requestFrame();
  }

  function onHashChange() {
    seekHash();
  }

  function onFocus(event) {
    const sceneIndex = sceneIndexFromTarget(event.target);
    if (enhanced && sceneIndex >= 0 && sceneIndex !== frame.sceneIndex) seek(sceneIndex);
  }

  function onBeforeMatch(event) {
    const sceneIndex = sceneIndexFromTarget(event.target);
    if (sceneIndex >= 0) seek(sceneIndex);
  }

  const api = {
    getState: () => ({ ...frame, enhanced }),
    subscribe,
    refresh,
    seek,
    manifest: STORY_SCENES,
  };

  window.BuildLandingStory = api;
  window.addEventListener("scroll", requestFrame, { passive: true });
  window.addEventListener("resize", onResize, { passive: true });
  window.addEventListener("hashchange", onHashChange);
  document.addEventListener("visibilitychange", onVisibilityChange);
  document.addEventListener("focusin", onFocus);
  document.addEventListener("beforematch", onBeforeMatch);
  reducedMotionQuery.addEventListener("change", onPreferenceChange);
  connection?.addEventListener?.("change", onPreferenceChange);
  setMode(profile, false);
  requestAnimationFrame(() => seekHash());

  return {
    ...api,
    destroy() {
      destroyed = true;
      subscribers.clear();
      window.removeEventListener("scroll", requestFrame);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("hashchange", onHashChange);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      document.removeEventListener("focusin", onFocus);
      document.removeEventListener("beforematch", onBeforeMatch);
      reducedMotionQuery.removeEventListener("change", onPreferenceChange);
      connection?.removeEventListener?.("change", onPreferenceChange);
      if (window.BuildLandingStory === api) delete window.BuildLandingStory;
    },
  };
}
