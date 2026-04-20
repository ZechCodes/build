import { state } from '../../state.js';
import { renderComplications } from '../../complications/render.js';

export function bindComplicationHandlers(instance, deviceId) {
  instance.addEventListener('complication_update', (evt) => {
    const comp = evt.detail;
    const channelId = comp.channel_id;
    if (!channelId) return;
    if (!state.complicationState.has(channelId)) state.complicationState.set(channelId, new Map());
    state.complicationState.get(channelId).set(comp.id, comp);
    if (state.chatCurrentChannel === channelId) renderComplications();
  });

  instance.addEventListener('complication_remove', (evt) => {
    const { channel_id, id } = evt.detail;
    const channelComps = state.complicationState.get(channel_id);
    if (channelComps) {
      channelComps.delete(id);
      if (state.chatCurrentChannel === channel_id) renderComplications();
    }
  });

  instance.addEventListener('complications', (evt) => {
    const { channel_id, complications } = evt.detail;
    if (!channel_id || !complications) return;
    if (!state.complicationState.has(channel_id)) state.complicationState.set(channel_id, new Map());
    const channelComps = state.complicationState.get(channel_id);
    for (const comp of complications) {
      channelComps.set(comp.id, comp);
    }
    if (state.chatCurrentChannel === channel_id) renderComplications();
  });

}
