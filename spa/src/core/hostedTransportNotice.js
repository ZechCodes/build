import { App } from "../appState.js";
import { onDevicesChanged } from "../devices.js";
import { desktopHttpValue } from "./httpDesktopEndpoint.js";
import { readUiRecord, subscribeUiRecords } from "./localUiStore.js";
import { uiAddress, updateUiState } from "./localUiState.js";
import { writeAccountId } from "./writeOperationRuntime.js";

export const HOSTED_HTTPS_DISCLOSURE = "Hosted HTTPS passes through Cloudflare. Cloudflare can read your code, prompts and terminal traffic on this connection.";
export const hostedNoticeAddress = (accountId, revision) =>
  uiAddress({ entityId: accountId, view: "account", kind: "hosted-https-notice", sub: String(revision) });

function currentPromotion() {
  const accountId = writeAccountId();
  const policy = App.deviceTransportPolicy;
  if (!accountId || policy?.accountId !== accountId || !policy.tunnelAccess || !(policy.promotionRevision > 0)) return null;
  const revision = policy.promotionRevision;
  return { accountId, revision, key: `${accountId}/${revision}/${App.accountEpoch}` };
}

export function mountHostedTransportNotice(host) {
  if (!host) return () => {};
  host.hidden = true;
  host.innerHTML = '<span role="status"></span><button type="button" class="btn">Acknowledge</button><span data-notice-error role="alert" hidden></span>';
  host.querySelector('[role="status"]').textContent = HOSTED_HTTPS_DISCLOSURE;
  const button = host.querySelector("button");
  const error = host.querySelector("[data-notice-error]");
  let stopRecord;
  let scope;
  let disposed = false;
  const follow = () => {
    const next = currentPromotion();
    if (next?.key === scope?.key) return;
    stopRecord?.();
    scope = next;
    host.hidden = true;
    error.hidden = true;
    button.disabled = false;
    button.onclick = null;
    if (!next) return;
    stopRecord = watchAcknowledgement(next, () => !disposed && scope === next
      && currentPromotion()?.key === next.key, { host, button, error });
  };
  const stopDevices = onDevicesChanged(follow);
  follow();
  return () => { disposed = true; stopDevices(); stopRecord?.(); host.hidden = true; };
}

async function readDesktopAcknowledgement(address, request, active) {
  if (!window.buildBridgeHttp?.hostedNotice) return;
  const cached = await readUiRecord(address);
  if (!active() || cached?.value === true) return;
  const accepted = desktopHttpValue(await window.buildBridgeHttp.hostedNotice(request));
  if (accepted) await updateUiState(address, () => true, active);
}

function watchAcknowledgement({ accountId, revision }, active, { host, button, error }) {
  const address = hostedNoticeAddress(accountId, revision);
  const request = { accountId, promotionRevision: revision };
  let readGeneration = 0;
  const paint = async () => {
    const generation = ++readGeneration;
    const record = await readUiRecord(address);
    if (active() && generation === readGeneration) host.hidden = record?.value === true;
  };
  const refresh = () => { void paint(); };
  const stop = subscribeUiRecords(address, refresh);
  void readDesktopAcknowledgement(address, request, active).catch(() => {}).then(refresh);
  button.onclick = async () => {
    if (!active()) return;
    button.disabled = true;
    error.hidden = true;
    try {
      if (window.buildBridgeHttp?.hostedNotice) {
        desktopHttpValue(await window.buildBridgeHttp.hostedNotice({ ...request, acknowledge: true }));
      }
      if (!await updateUiState(address, () => true, active)) throw new Error("Notice acknowledgement did not commit");
      await paint();
    } catch {
      if (active()) { error.textContent = "Could not save your acknowledgement. Try again."; error.hidden = false; }
    } finally {
      if (active()) button.disabled = false;
    }
  };
  return stop;
}
