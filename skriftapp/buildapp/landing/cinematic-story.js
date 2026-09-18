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

function canEnhance(profile, reducedMotion, saveData, forcedStatic) {
  return profile !== "static" && !reducedMotion && !saveData && !forcedStatic;
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

function opacityForScene(index, frame) {
  if (index === frame.sceneIndex) {
    return frame.local > 0.8 && index < STORY_SCENES.length - 1
      ? 1 - (frame.local - 0.8) / 0.2
      : 1;
  }
  if (index === frame.sceneIndex + 1 && frame.local > 0.8) return (frame.local - 0.8) / 0.2;
  return 0;
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

function applyFrame({ frame, stage, scenes, progressElement, positionElement }) {
  stage.dataset.scene = frame.scene;
  stage.dataset.profile = frame.profile;
  stage.dataset.checkpoint = frame.checkpoint;
  stage.style.setProperty("--story-progress", String(frame.progress));
  stage.style.setProperty("--scene-local", String(frame.local));

  scenes.forEach((scene, index) => {
    const isActive = index === frame.sceneIndex;
    scene.style.setProperty("--scene-opacity", String(clamp(opacityForScene(index, frame))));
    scene.classList.toggle(ACTIVE_CLASS, isActive);
    setSceneInteractive(scene, isActive);
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
  stage.style.removeProperty("--story-progress");
  stage.style.removeProperty("--scene-local");
  scenes.forEach((scene) => {
    scene.style.removeProperty("--scene-opacity");
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
  let forcedStatic = false;
  let staticReason = "";
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

  function preserveAfterStory(offsetFromEnd, defer = false) {
    const restore = () => window.scrollTo({
      top: storyTop(story) + story.offsetHeight + offsetFromEnd,
      behavior: "auto",
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
      requestAnimationFrame(() => scenes[previous.sceneIndex]?.scrollIntoView({ block: "start" }));
    } else if (preserveFrame && reader.afterPinnedStory) {
      preserveAfterStory(reader.offsetFromEnd, true);
    }
  }

  function useEnhancedStory(previous, reader, preserveFrame) {
    story.style.setProperty("--story-travel", String(storyTravel(profile)));
    story.style.setProperty("--story-height", `${(storyTravel(profile) + 1) * stageHeight(stage)}px`);
    if (preserveFrame && reader.inPinnedStory) {
      const travel = travelAtFrame(previous.sceneIndex, previous.local, profile);
      window.scrollTo({ top: storyTop(story) + travel * stageHeight(stage), behavior: "auto" });
    } else if (preserveFrame && reader.afterPinnedStory) {
      preserveAfterStory(reader.offsetFromEnd);
    }
    measureFrame();
  }

  function setMode(nextProfile, preserveFrame = false) {
    const previous = frame;
    const wasEnhanced = enhanced;
    const reader = readerPosition(story, stage);
    const shouldEnhance = canEnhance(nextProfile, reducedMotionQuery.matches, currentSaveData(), forcedStatic);
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

  function useStatic(reason = "runtime-fallback") {
    forcedStatic = true;
    staticReason = reason;
    refresh({ preserve: true });
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

  function onSelectionChange() {
    if (!enhanced) return;
    const selectionTarget = document.getSelection()?.anchorNode?.parentElement;
    const sceneIndex = sceneIndexFromTarget(selectionTarget);
    if (sceneIndex >= 0 && sceneIndex !== frame.sceneIndex) seek(sceneIndex);
  }

  const api = {
    getState: () => ({ ...frame, enhanced, staticReason }),
    subscribe,
    refresh,
    seek,
    useStatic,
    manifest: STORY_SCENES,
  };

  window.BuildLandingStory = api;
  window.addEventListener("scroll", requestFrame, { passive: true });
  window.addEventListener("resize", onResize, { passive: true });
  window.addEventListener("hashchange", onHashChange);
  document.addEventListener("visibilitychange", onVisibilityChange);
  document.addEventListener("focusin", onFocus);
  document.addEventListener("beforematch", onBeforeMatch);
  document.addEventListener("selectionchange", onSelectionChange);
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
      document.removeEventListener("selectionchange", onSelectionChange);
      reducedMotionQuery.removeEventListener("change", onPreferenceChange);
      connection?.removeEventListener?.("change", onPreferenceChange);
      if (window.BuildLandingStory === api) delete window.BuildLandingStory;
    },
  };
}
