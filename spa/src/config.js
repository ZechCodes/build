// Build-time configuration. Set VITE_RELAY_URL when building for an environment
// where the relay is not the local dev one (production: wss://relay.getbuild.ing).
export const RELAY_URL = import.meta.env.VITE_RELAY_URL || "ws://localhost:18090";
