// The hardware's finish at runtime: classic silver aluminium in place of the
// exported space black and deep blue, so the devices stand off the black page
// without a glow or an outline. Keys, glass, sensors and screens stay dark;
// the UI on the screens is untouched. Colours are linear, as three.js holds
// them; the Blender fixture follows these numbers.
export const DEVICE_FINISH = Object.freeze({
  SpaceBlackAluminum: { color: [0.6, 0.61, 0.63], metalness: 1, roughness: 0.34 },
  MachinedSpaceBlackEdge: { color: [0.72, 0.73, 0.75], metalness: 1, roughness: 0.2 },
  TrackpadSpaceBlack: { color: [0.52, 0.53, 0.55], metalness: 0.9, roughness: 0.26 },
  DeepBlueAluminum: { color: [0.6, 0.61, 0.63], metalness: 1, roughness: 0.34 },
  DeepBlueMachinedEdge: { color: [0.72, 0.73, 0.75], metalness: 1, roughness: 0.2 },
  DeepBlueCeramicShield: { color: [0.7, 0.71, 0.72], metalness: 0.15, roughness: 0.38 },
  CameraRing: { color: [0.7, 0.71, 0.73], metalness: 1, roughness: 0.16 },
});

export function applyDeviceFinish(material) {
  const finish = DEVICE_FINISH[material.name];
  if (!finish) return material;
  material.color.setRGB(...finish.color);
  material.metalness = finish.metalness;
  material.roughness = finish.roughness;
  return material;
}
