import "../styles/agentObservation.css";
import { el, setHidden } from "../dom.js";
import { patchElement } from "./domPatch.js";
import { observationPanelModel } from "./agentObservationModel.js";
import { observationPanelHtml } from "./agentObservationRender.js";

export function mountAgentObservation(host) {
  let currentGeneration;

  return {
    set(surfaces, { generation = null, working = false } = {}) {
      const html = observationPanelHtml(observationPanelModel(surfaces, { working }));
      if (!html) {
        if (host.firstChild) host.replaceChildren();
        setHidden(host, true);
        return;
      }
      const next = el(html);
      const changedGeneration = currentGeneration !== undefined && generation !== currentGeneration;
      if (!host.firstElementChild || changedGeneration) host.replaceChildren(next);
      else patchElement(host.firstElementChild, next);
      currentGeneration = generation;
      setHidden(host, false);
    },
    dispose() {
      host.replaceChildren();
      host.hidden = true;
      currentGeneration = undefined;
    },
  };
}
