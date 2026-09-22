// What harnesses one machine offers: the bridge's models.list, held on the
// context that asked for it.
//
// The catalog is a property of the bridge, not of the account — two machines on
// one account can be running different releases and offer different agents — so
// every device holds its own, and a surface that is about one machine asks that
// machine's context rather than an ambient cache. It is read once per device
// and kept: the answer only moves when the bridge's settings do, and the
// Settings page says so by refreshing.

import { normalizeModelCatalog } from "./modelPicker.js";
import { deviceModelsAddress, watchSettingsRecord } from "./settingsRecords.js";

/** What a bridge that cannot be asked offers: the harness's own default, and
 *  nothing to choose between. An older bridge without the RPC answers the same,
 *  which is exactly what it supports. */
export const EMPTY_CATALOG = Object.freeze(normalizeModelCatalog({ models: [], efforts: [] }));

/** What a surface offers while the machine it asked has yet to answer — no
 *  list, and no default named for it. Not the same fact as EMPTY_CATALOG: that
 *  one is an answer, and this is the wait for one. */
export const UNASKED_CATALOG = Object.freeze({ default_provider: "", providers: [] });

/**
 * The catalog half of a device context: `modelCatalog()` and
 * `refreshModelCatalog()`, closing over the context so a reconnect's new
 * transport is the one the next read goes out on.
 *
 * Every write is guarded by `context.active()`: a read can outlive the device
 * it was asked of, and its answer still belongs to whoever asked, but it must
 * not become the catalog of anything afterwards.
 *
 * The read goes out on `context.rpc`, so a machine that is away — or whose
 * bridge speaks an API major nothing here claims — refuses it the way it
 * refuses every other read, rather than being asked for a list this tab could
 * not read the answer to. A refusal reads as the empty catalog below, which is
 * what such a machine offers.
 */
export function createModelCatalog(context) {
  let held = null;
  let asking = null;
  let asked = false;
  let signature = null;
  const listeners = new Set();
  const record = watchSettingsRecord(deviceModelsAddress(context.deviceId), (catalog) => {
    const nextSignature = catalog ? JSON.stringify(catalog) : null;
    if (nextSignature === signature) return;
    signature = nextSignature;
    held = catalog ? normalizeModelCatalog(catalog) : null;
    for (const listener of [...listeners]) listener(held);
  });

  const read = async () => {
    asked = true;
    await record.pull(async () => {
      const answer = await context.rpc("models.list");
      if (!context.active()) throw new Error("device retired during models.list");
      return answer;
    });
    return held || EMPTY_CATALOG;
  };

  const readQuietly = () => read().catch(() => held || EMPTY_CATALOG);

  const askOnce = () => {
    if (!asking) asking = readQuietly().finally(() => { asking = null; });
    return asking;
  };

  return {
    /** This device's catalog, asked for once. */
    async modelCatalog() {
      await record.read();
      if (held) {
        if (!asked) void askOnce();
        return held;
      }
      return askOnce();
    },

    /** Ask again, and keep the new answer: the account changed something the
     *  catalog reports, so what was held is out of date. Refuses out loud —
     *  the page that asked is the one that can say the refresh did not land. */
    refreshModelCatalog() {
      return read();
    },
    onModelCatalogChanged(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    disposeModelCatalog() {
      record.dispose();
      listeners.clear();
    },
  };
}
