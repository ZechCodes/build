// Shell bootstrap. See planning/dashboard-v2/06-shell.md.

import { log } from '../core/log.js';
import { buildLayout } from './layout.js';
import { router } from './router.js';
import { ChannelPanelView } from './channel-panel.js';
import { RailView } from './rail.js';
import { TabBarView } from './tabs.js';
import { SidebarSectionsView } from './sidebar.js';
import { DropdownView } from './dropdown.js';
import { TopBarView } from './top-bar.js';

const plog = log('shell');

export function initShell(root) {
  plog.info('init');
  buildLayout(root);

  const views = [
    new ChannelPanelView(),
    new TopBarView(),
    new TabBarView(),
    new RailView(),
    new SidebarSectionsView(),
    new DropdownView(),
  ];
  for (const v of views) v.activate();

  router.init();

  return { views, router };
}
