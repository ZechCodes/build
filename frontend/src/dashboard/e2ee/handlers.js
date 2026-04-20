// Orchestrator. Each sub-module owns one domain of the 32 SSE events emitted
// by vendor/e2ee.js's _handleDataFrame. Splitting them keeps every file under
// ~250 lines and makes the per-domain dependencies (channels, messages, files,
// terminal, complications, agent activity) explicit at the import boundary.

import { bindLifecycleHandlers } from './handlers/lifecycle.js';
import { bindChannelHandlers } from './handlers/channels.js';
import { bindMessageHandlers } from './handlers/messages.js';
import { bindAgentHandlers } from './handlers/agent.js';
import { bindTerminalHandlers } from './handlers/terminal.js';
import { bindFileHandlers } from './handlers/files.js';
import { bindComplicationHandlers } from './handlers/complications.js';

export function bindE2EEEvents(instance, deviceId) {
  if (!instance) return;
  bindLifecycleHandlers(instance, deviceId);
  bindChannelHandlers(instance, deviceId);
  bindMessageHandlers(instance, deviceId);
  bindAgentHandlers(instance, deviceId);
  bindTerminalHandlers(instance, deviceId);
  bindFileHandlers(instance, deviceId);
  bindComplicationHandlers(instance, deviceId);
}
