// Metadata-only presentation for staged and sent secrets, plus the one modal
// that ever holds a live value. Values stay in the password field and the
// immediate stage request; neither chip markup nor controller state retains
// one after submission or dismissal.

import { fieldTraits } from "./fieldTraits.js";
import { ICON_LOCK, ICON_X } from "./icons.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { secretChipLabel } from "./secretLabels.js";
import { esc } from "./text.js";

export { secretChipLabel } from "./secretLabels.js";

const MIN_TTL_SECONDS = 60;
const MAX_TTL_SECONDS = 2_592_000;
const NAME_PATTERN = /^[A-Za-z_][A-Za-z0-9_.-]{0,63}$/;

export const validSecretName = (name) => NAME_PATTERN.test(String(name || ""));

const expiryError = () => new Error("Choose a valid expiry between 60 and 2592000 seconds");

/** The modal's selection as the exact nullable seconds the wire accepts. */
export function secretTtlSeconds(choice, customSeconds) {
  if (choice === "none") return null;
  const seconds = choice === "custom" ? Number(customSeconds) : Number(choice);
  const exactInteger = String(seconds) === String(choice === "custom" ? customSeconds : choice).trim();
  if (!exactInteger || !Number.isSafeInteger(seconds) || seconds < MIN_TTL_SECONDS || seconds > MAX_TTL_SECONDS) {
    throw expiryError();
  }
  return seconds;
}

export const secretComposerAvailable = (cachedSupport, threadId) => cachedSupport === true && Boolean(threadId);

const secretChipHtml = (metadata, removable) => {
  const label = secretChipLabel(metadata);
  const id = metadata?.secret_id || "";
  const remove = removable && id
    ? `<button type="button" class="composer-chip-remove" data-secret-remove="${esc(id)}" aria-label="Remove ${esc(metadata.name || "secret")}">${ICON_X}</button>`
    : "";
  return `<div class="composer-chip secret-chip" data-secret-id="${esc(id)}">
    <span class="secret-chip-icon" aria-hidden="true">${ICON_LOCK}</span>
    <span class="secret-chip-label">${esc(label)}</span>${remove}
  </div>`;
};

export const secretChipsHtml = (entries = [], { removable = true } = {}) =>
  entries.map((entry) => secretChipHtml(entry, removable)).join("");

const selected = (held, wanted) => held === wanted ? " selected" : "";

function secretModalBodyHtml({ scope = "agent", ttl = "none", busy = false }) {
  return `<h3 id="secret-modal-title">Attach a secret</h3>
    <p class="sub secret-modal-intro">The value is sealed to this device and is not included in the message.</p>
    <label class="secret-field-label" for="secret-key">Name</label>
    <input id="secret-key" name="secret-key" type="text" maxlength="64" placeholder="TEST_TOKEN" ${fieldTraits("identifier")} />
    <label class="secret-field-label" for="secret-value">Value</label>
    <input id="secret-value" name="secret-value" type="password" ${fieldTraits("identifier")} />
    <fieldset class="secret-scope">
      <legend>Scope</legend>
      <label><input type="radio" name="secret-scope" value="agent"${scope === "agent" ? " checked" : ""} />
        <span><strong>Agent</strong><small>Limited to this agent’s conversation.</small></span>
      </label>
      <label><input type="radio" name="secret-scope" value="project"${scope === "project" ? " checked" : ""} />
        <span><strong>Project</strong><small>Available to current agents in this project.</small></span>
      </label>
    </fieldset>
    <label class="secret-field-label" for="secret-ttl">Expiry</label>
    <select id="secret-ttl" name="secret-ttl">
      <option value="none"${selected(ttl, "none")}>No expiry</option>
      <option value="3600"${selected(ttl, "3600")}>1 hour</option>
      <option value="86400"${selected(ttl, "86400")}>1 day</option>
      <option value="604800"${selected(ttl, "604800")}>7 days</option>
      <option value="custom"${selected(ttl, "custom")}>Custom</option>
    </select>
    <input id="secret-custom-ttl" name="secret-custom-ttl" type="number" min="60" max="2592000" step="1" placeholder="Seconds" ${fieldTraits("identifier")} ${ttl === "custom" ? "" : "hidden"} />
    <p class="sub secret-ttl-note">The expiry starts when the message is sent.</p>
    <div class="warn secret-modal-error" data-secret-error aria-live="polite" hidden></div>
    <div class="row secret-modal-actions">
      <button type="button" class="btn" data-secret-cancel${busy ? " disabled" : ""}>Cancel</button>
      <button type="button" class="btn primary" data-secret-submit${busy ? " disabled" : ""}>${busy ? "Attaching…" : "Attach secret"}</button>
    </div>`;
}

export function secretModalHtml(state = {}) {
  return modalDialogHtml(secretModalBodyHtml(state), { className: "modal-secret" });
}

const nameError = "Use 1–64 letters, numbers, _, . or -; start with a letter or _.";

function modalSubmission(body, value, ttlField, customTtlField, clearValue) {
  const name = body.querySelector("#secret-key").value.trim();
  if (!validSecretName(name)) throw new Error(nameError);
  if (!value) throw new Error("Enter a secret value.");
  const scope = body.querySelector('[name="secret-scope"]:checked')?.value || "agent";
  return {
    name,
    value,
    scope,
    ttlSeconds: secretTtlSeconds(ttlField.value, customTtlField.value),
    clearValue,
  };
}

/** Open the only surface that handles a live secret value. */
export function openSecretModal({ stage, onChange = () => {}, host = document.body } = {}) {
  if (typeof stage !== "function") throw new Error("Secret modal requires a stage action");
  let busy = false;
  let dismissed = false;
  let modal;
  let body;
  let valueField;

  const wipeValue = () => {
    if (valueField) valueField.value = "";
  };
  const closed = () => {
    dismissed = true;
    wipeValue();
  };
  modal = openModal({
    dialogHtml: secretModalHtml(),
    scrimId: "secret-scrim",
    host,
    initialFocus: "#secret-key",
    canDismiss: () => !busy,
    onClose: closed,
  });
  body = modal.body;
  body.setAttribute("aria-labelledby", "secret-modal-title");
  valueField = body.querySelector("#secret-value");

  const errorField = body.querySelector("[data-secret-error]");
  const submitButton = body.querySelector("[data-secret-submit]");
  const cancelButton = body.querySelector("[data-secret-cancel]");
  const ttlField = body.querySelector("#secret-ttl");
  const customTtlField = body.querySelector("#secret-custom-ttl");
  const setError = (message) => {
    errorField.textContent = message;
    errorField.hidden = !message;
  };
  const setBusy = (next) => {
    busy = next;
    submitButton.disabled = next;
    cancelButton.disabled = next;
    submitButton.textContent = next ? "Attaching…" : "Attach secret";
  };
  const close = () => {
    if (busy) return null;
    wipeValue();
    dismissed = true;
    return modal.close();
  };

  ttlField.onchange = () => {
    customTtlField.hidden = ttlField.value !== "custom";
    if (!customTtlField.hidden) customTtlField.focus();
  };
  cancelButton.onclick = close;

  const submit = async () => {
    if (busy) return;
    let value = valueField.value;
    wipeValue();
    setError("");
    let input;
    try {
      input = modalSubmission(body, value, ttlField, customTtlField, wipeValue);
    } catch (error) {
      value = "";
      setError(error.message);
      return;
    }
    setBusy(true);
    try {
      const staged = await stage(input);
      if (!staged) throw new Error("The secret could not be attached");
      value = "";
      if (dismissed) return;
      onChange();
      setBusy(false);
      await close();
    } catch (error) {
      value = "";
      if (dismissed) return;
      setBusy(false);
      setError(error?.message || "The secret could not be attached");
    } finally {
      value = "";
      wipeValue();
    }
  };
  submitButton.onclick = submit;
  valueField.onkeydown = (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    void submit();
  };
  return { body, close, submit };
}

/** Bind one conversation's metadata-only draft service to composer chrome. */
export function createSecretComposer({ service, onChange = () => {}, onError = () => {}, host = document.body } = {}) {
  if (!service || typeof service.stage !== "function" || typeof service.remove !== "function" || typeof service.entries !== "function") {
    throw new Error("Secret composer requires a draft service");
  }
  let modal = null;
  let disposed = false;
  const open = () => {
    if (disposed) return null;
    modal = openSecretModal({
      stage: (input) => service.stage(input),
      onChange,
      host,
    });
    return modal;
  };
  const wire = (root) => {
    for (const button of root?.querySelectorAll?.("[data-secret-remove]") || []) {
      button.onclick = async () => {
        button.disabled = true;
        try {
          const revoked = await service.remove(button.dataset.secretRemove);
          if (!revoked) throw new Error("The secret could not be revoked");
          if (!disposed) onChange();
        } catch (error) {
          if (!disposed) {
            button.disabled = false;
            onError(error);
          }
        }
      };
    }
  };
  return Object.freeze({
    open,
    wire,
    chipsHtml: () => secretChipsHtml(service.entries()),
    entries: () => service.entries(),
    dispose() {
      disposed = true;
      modal?.close();
      modal = null;
    },
  });
}
