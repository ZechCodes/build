// Shell bootstrap.

import { log } from '../core/log.js';
import { buildLayout } from './layout.js';
import { router } from './router.js';
import { ChannelPanelView } from './channel-panel.js';
import { RailView } from './rail.js';
import { SidebarSectionsView } from './sidebar.js';
import { DropdownView } from './dropdown.js';
import { TasksView } from './tasks.js';
import { ComplicationsView } from './complications.js';
import { OverlayView } from './overlay.js';
import { ImageLightboxView } from './image-lightbox.js';
import { ReconnectPillView } from './reconnect-pill.js';
import { AccountView } from './account.js';

const plog = log('shell');

export function initShell(root) {
  plog.info('init');
  buildLayout(root);

  const views = [
    new ChannelPanelView(),
    new RailView(),
    new SidebarSectionsView(),
    new DropdownView(),
    new TasksView(),
    new ComplicationsView(),
    new OverlayView(),
    new ImageLightboxView(),
    new ReconnectPillView(),
    new AccountView(),
  ];
  for (const v of views) v.activate();

  router.init();

  return { views, router };
}
