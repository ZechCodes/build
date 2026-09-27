// Identity checks mount real task components and replace only connection
// recovery, which otherwise requires an authenticated device session.
export const deviceShim = {
  name: "task-identity-device-recovery-shim",
  enforce: "pre",
  resolveId(source, importer) {
    return source === "./deviceReconnect.js" && importer?.includes("/src/core/")
      ? "\0task-identity-device-recovery" : null;
  },
  load(id) {
    if (id !== "\0task-identity-device-recovery") return null;
    return `export const onDeviceMoved=()=>()=>{};
      export const deviceIsReconnecting=()=>false;
      export const deviceIsAway=()=>false;
      export const onDeviceReachable=()=>()=>{};
      export const deviceSession=()=>null;
      export const deviceWatch=()=>({away:()=>false,reconnecting:()=>false,moved:()=>()=>{}});`;
  },
};
