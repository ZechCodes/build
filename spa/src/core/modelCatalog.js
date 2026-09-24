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
 *
 * What the machine last offered is a record like any other: a surface stood up
 * over a machine that has not answered yet (core/surfaceContext.js) is handed
 * it from disk, and the machine is asked only once it can be (`canAsk`) —
 * which, for one that could not when a surface first wanted it, is when its
 * bridge greets (`answering`). Listeners hear the answer land.
 *
 * The ask itself waits for the greeting of the session it goes out on
 * (`answersOnceGreeted`), and an answer is kept only if the machine can still
 * be read on that same session when it lands: a bridge this tab cannot read
 * never has its catalog written over what the disk held.
 */
export function createModelCatalog(context, {
  canAsk = () => true,
  answersOnceGreeted = async () => canAsk(),
} = {}) {
  let held = null;
  let asking = null;
  let asked = false;
  let wanted = false;
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
    await record.pull(async () => {
      if (!(await answersOnceGreeted())) throw new Error("the machine cannot be asked for its catalog");
      const session = context.session;
      asked = true;
      const answer = await context.rpc("models.list");
      if (!context.active()) throw new Error("device retired during models.list");
      if (context.session !== session || !canAsk()) throw new Error("the bridge that answered models.list cannot be read");
      return answer;
    });
    return held || EMPTY_CATALOG;
  };

  const readQuietly = () => read().catch(() => held || EMPTY_CATALOG);

  const askOnce = () => {
    if (!canAsk()) return Promise.resolve(held || EMPTY_CATALOG);
    if (!asking) asking = readQuietly().finally(() => { asking = null; });
    return asking;
  };

  return {
    /** This device's catalog, asked for once. */
    async modelCatalog() {
      wanted = true;
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
    /** The machine's bridge has greeted: ask it now if a surface wanted its
     *  catalog while it could not be asked. */
    answering() {
      if (wanted && !asked) void askOnce();
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
