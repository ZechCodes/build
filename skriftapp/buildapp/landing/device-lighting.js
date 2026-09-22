// Reflected studio strips shape the aluminum, with dark space between them.
// This scene is baked once; the light cards never appear on the page.
export function createDeviceEnvironment(three, renderer) {
  const { Color, Mesh, MeshBasicMaterial, PlaneGeometry, PMREMGenerator, Scene } = three;
  const studio = new Scene();
  studio.background = new Color(0x111316);
  const cards = [
    { size: [3.5, 5], position: [-4, 3, 5], color: 0xfffcf6, intensity: 4.0 },
    { size: [0.85, 5], position: [4, 1, 3], color: 0xf1f4fa, intensity: 1.7 },
    { size: [5, 1.1], position: [0, 5, 1], color: 0xffffff, intensity: 3.2 },
    { size: [6, 2], position: [-1, 0.1, 5], color: 0xf0f3f7, intensity: 0.9 },
    { size: [1.2, 4], position: [-3, 1, -4], color: 0xffffff, intensity: 2.0 },
  ];
  for (const { size, position, color, intensity } of cards) {
    const material = new MeshBasicMaterial({ color: new Color(color).multiplyScalar(intensity) });
    const card = new Mesh(new PlaneGeometry(...size), material);
    card.position.set(...position);
    card.lookAt(0, 0, 0);
    studio.add(card);
  }
  const generator = new PMREMGenerator(renderer);
  const target = generator.fromScene(studio, 0.02);
  generator.dispose();
  for (const card of studio.children) {
    card.geometry.dispose();
    card.material.dispose();
  }
  return target;
}
