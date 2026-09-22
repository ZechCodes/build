# Device and brand references

The models are original Blender geometry. Apple product photographs were used to study proportions and construction; they are not included in the repository or served by the page. The devices show Build demonstration screens and carry no Apple marks.

## Hardware

| Model | Primary reference | Details used |
| --- | --- | --- |
| Laptop | [14-inch MacBook Pro specifications](https://www.apple.com/macbook-pro/specs/) | M5 model; 312.6 × 221.2 × 15.5 mm closed body; 3024 × 1964 display; black glass bezel and camera notch; 78-key ANSI keyboard, speaker fields, Force Touch trackpad, hinge, MagSafe, Thunderbolt, headphone, HDMI, and SDXC details. |
| Tablet | [iPad Pro specifications](https://www.apple.com/ipad-pro/specs/) and [Apple Developer dimensional drawing](https://developer.apple.com/download/files/accessories/dimensional-drawings/ipad-pro-11-inch-m5.pdf) | 11-inch M5 model; 249.7 × 177.5 × 5.3 mm body; 2420 × 1668 at 264 ppi; even bezel; landscape front camera; top/volume controls, rear camera, Smart Connector, and Thunderbolt/USB-C. |
| Phone | [iPhone 17 Pro Max specifications](https://support.apple.com/en-my/125091), [Apple Developer dimensional drawing](https://developer.apple.com/download/files/accessories/dimensional-drawings/iphone-17-pro-max.pdf), and [Apple launch photography](https://www.apple.com/newsroom/2025/09/apple-unveils-iphone-17-pro-and-iphone-17-pro-max/) | 2025 Pro Max; 78 × 163.4 × 8.75 mm body; 1320 × 2868 at 460 ppi; Dynamic Island; Action, volume, side, and Camera Control buttons; USB-C; deep-blue brushed aluminum unibody, Ceramic Shield back inset, full-width forged plateau, three camera rings, flash, and LiDAR. |

The hardware is a proportion study rather than a CAD replica. The enclosure models target the published nominal dimensions and use outward-facing shell geometry. Camera and control projections extend beyond the published bare-body depth and are reflected in the generated overall bounds. Planar corner radii are independent of slab thickness, so the thin bodies retain their rounded silhouettes. The iPad body uses a 15.05 mm outer corner radius and its display uses a 6.6 mm radius; their sampled corner centers remain concentric across the roughly 8.5 mm bezel.

The active display rectangles come from Apple's resolution and pixel density: 3024 × 1964 at 254 ppi, 2420 × 1668 at 264 ppi, and 1320 × 2868 at 460 ppi. This makes the modeled screen aspect exactly match each native fixture. Each active plane sits measurably in front of its cover glass to avoid coincident surfaces. The MacBook display assembly is attached to a named hinge at the rear edge: 90° rotation is closed and −15° is the authored position, giving 105° of travel. The keyboard uses raised, tapered key bodies with a shallow 0.10 mm dish over a cut recess, rather than flat marks on the deck. Generated body dimensions, projected bounds, screen clearances, transformed screen corners, and hinge endpoints are recorded in `assets/devices/metadata.json` and generated `device-contract.js`; those files are the rendering contract.

The models contain no Apple marks. Space-black and deep-blue aluminum uses metallic 1.0 and clearcoat 0.0 so reflected light strips, separated by dark regions, define the metal without a coated-plastic highlight. Glass, lenses, keys, and the phone's rear insert retain distinct material responses. All geometry and materials are original and no Apple photographs or CAD files are included in generated assets.

## Brand

The mint “b” mark and application-icon variants come from the existing Build branding assets in commit `a96359e6` (`Bundle official brand assets and update application icons`). The canonical source is `assets/brand/build-mark.svg`. The header uses the transparent mint mark; the favicon uses the black-on-mint square variant. The visible wordmark is `build`, without a trailing underscore.
