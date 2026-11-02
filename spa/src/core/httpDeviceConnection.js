import {
  adoptDeviceConnection, blockDeviceConnection, closeQuietly, deviceContextEra, deviceContextIdentity,
} from "./deviceContexts.js";
import { httpEndpointFor, httpResourceRoutes } from "./httpConnections.js";
import { openHttpSession } from "./httpSession.js";
import { dispatchChangeEvent } from "./changeEvents.js";

// Shares the existing device attempt and lifetime owners. An HTTP stream is
// independent of terminal RTC; its failure replaces only this exact app bundle.
export async function openHttpDeviceConnection(deviceId, attempt, { greet, onLost, onAdopt, onDetached }) {
  const era = deviceContextEra();
  const identity = deviceContextIdentity(deviceId);
  let lifetime = null;
  const current = () => deviceContextEra() === era && deviceContextIdentity(deviceId) === identity
    && (lifetime ? lifetime.current() : attempt.isCurrent());
  const endpoint = await httpEndpointFor(deviceId, current);
  if (!attempt.own(endpoint, (held) => held.close())) throw new Error("HTTP attempt superseded");
  const session = await openHttpSession({ endpoint, routes: httpResourceRoutes(), isCurrent: current,
    onPush: (frame) => { if (current()) dispatchChangeEvent(frame, deviceId); },
    onLost: (error) => { if (lifetime?.current()) onLost(lifetime, error); },
  });
  if (!attempt.own(session, closeQuietly)) throw new Error("HTTP attempt superseded");
  const adopted = adoptDeviceConnection(session, null, onDetached);
  lifetime = adopted.lifetime;
  attempt.release(endpoint); attempt.release(session);
  attempt.own(lifetime, () => blockDeviceConnection(deviceId, lifetime, "http-lost"));
  try {
    await greet(adopted.context, { suppressFailure: false, isAuthoritative: current, lifetime });
    if (!current()) throw new Error("HTTP attempt superseded");
    onAdopt(adopted.context);
    attempt.release(lifetime);
    return adopted.context;
  } catch (error) {
    blockDeviceConnection(deviceId, lifetime, error.blockedReason || "http-failed");
    throw error;
  }
}
