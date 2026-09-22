// A capture's full record, separate from the board row that only summarizes it.
// The decision page and the client's pending row use this same address, so a
// reply from either surface wakes the other through the local cache.

export const captureRecordAddress = (deviceId, captureId) => ({
  deviceId: deviceId || "",
  entityId: captureId,
  kind: "capture",
});
