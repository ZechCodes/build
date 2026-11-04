// 3.33 advertised device recovery before its owner pin and pooled directory
// sessions were complete. Only 3.34 can replace every legacy projection writer.
import { satisfies } from "./bridgeApi/semver.js";

export const supportsDeviceSync = (capabilities, version) => capabilities?.sync?.device === true
  && satisfies(version, ">=3.34.0");
