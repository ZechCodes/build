// Build-time configuration. Set VITE_RELAY_URL when building for an environment
// where the relay is not the local dev one (production: wss://relay.getbuild.ing).
export const RELAY_URL = import.meta.env.VITE_RELAY_URL || "ws://localhost:18090";

// An explicit channel marks the deployed dev stack; local Vite mode is separate.
export const BUILD_CHANNEL = import.meta.env.VITE_BUILD_CHANNEL || "prod";
