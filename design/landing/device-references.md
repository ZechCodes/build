# Device and brand references

The models are original Blender geometry. Apple product photographs were used to study proportions and construction; they are not included in the repository or served by the page. The devices show Build demonstration screens and carry no Apple marks.

## Hardware

| Model | Primary reference | Details used |
| --- | --- | --- |
| Laptop | [14-inch MacBook Pro specifications and photographs](https://www.apple.com/macbook-pro/specs/) | 312.6 × 221.2 mm footprint; 3024:1964 panel; thin rounded lid; small camera notch; recessed keyboard, speaker grilles, and a large flush trackpad. |
| Tablet | [13-inch iPad Pro specifications and photographs](https://www.apple.com/ipad-pro/specs/) | 281.6 × 215.5 × 5.1 mm body; 4:3 panel; even black bezel; rounded glass corners and narrow flat metal sides. |
| Phone | [iPhone 17 Pro technical specifications and front/back photograph](https://support.apple.com/en-my/125090) | 71.9 × 150 × 8.75 mm body; 1206:2622 panel; rounded display corners, narrow bezel, flat frame, and a small camera island. |

The hardware is a proportion study rather than a CAD replica. Planar corner radii are independent of slab thickness, so thin bodies retain their rounded silhouettes. Small edge bevels provide a highlight without inflating the frame. The runtime laptop retains its keyboard geometry. Satin graphite materials and broad neutral lighting keep the hardware visible on the dark page.

The tablet has dedicated 4:3 screen maps. The runtime preserves each source image's proportions when its aspect ratio differs slightly from the modeled panel. Generated dimensions and screen corners are recorded in `assets/devices/metadata.json` and the generated `device-contract.js`; those files are the rendering contract.

## Brand

The mint “b” mark and application-icon variants come from the existing Build branding assets in commit `a96359e6` (`Bundle official brand assets and update application icons`). The canonical source is `assets/brand/build-mark.svg`. The header uses the transparent mint mark; the favicon uses the black-on-mint square variant. The visible wordmark is `build`, without a trailing underscore.
