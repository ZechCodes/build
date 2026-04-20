// Bridges Skrift's `sk:notification-status` DOM event onto the bus as
// `sse.connected` / `sse.disconnected`.

import { bus } from '../core/bus.js';
import { log } from '../core/log.js';

const plog = log('sse');

export function bindSse() {
  document.addEventListener('sk:notification-status', (evt) => {
    const status = evt.detail?.status;
    if (status === 'connected') {
      plog.debug('connected');
      bus.emit('sse.connected', {});
    } else if (status === 'disconnected') {
      plog.debug('disconnected');
      bus.emit('sse.disconnected', {});
    }
  });
}
