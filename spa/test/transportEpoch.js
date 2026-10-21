// Tests concerned only with carriers/correlation inject a fence double. Epoch
// authority tests use createBridgeEpoch and the published stamped fixtures.
export const transportEpoch = () => ({
  current: () => null,
  accepts: () => true,
  acceptGreeting: () => true,
  onChange: () => () => {},
  onUnexpected: () => () => {},
});
