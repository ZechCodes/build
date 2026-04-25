// Image lightbox — clicks on a rendered <build-image>'s <img> open a
// full-screen zoomable viewer. Wheel (desktop) / pinch (touch) / double-
// tap zoom. Drag pans when zoomed. Esc / backdrop / × close.
//
// Mounted once at shell init. Uses a single .v2-lightbox root created
// on activate() and lazy-filled when open. The underlying transform is
// applied inline on every state change so we can anchor zoom at the
// cursor / centroid (not the image center).

const MIN_SCALE = 0.25;
const MAX_SCALE = 8;
const DBLCLICK_SCALE = 2.5;

function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)); }

export class ImageLightboxView {
  constructor() {
    this.root = null;
    this.imgEl = null;
    this.pathEl = null;
    this.triggerEl = null;
    this.scale = 1;
    this.tx = 0;
    this.ty = 0;
    this.pointers = new Map();    // pointerId → {x, y}
    this._pinchStartDist = 0;
    this._pinchStartScale = 1;
    this._panLast = null;
    this._bound = false;
  }

  activate() {
    if (this._bound) return;
    this._bound = true;
    document.addEventListener('click', this._onDocClick);
    document.addEventListener('keydown', this._onKey);
  }

  deactivate() {
    if (!this._bound) return;
    this._bound = false;
    document.removeEventListener('click', this._onDocClick);
    document.removeEventListener('keydown', this._onKey);
    this._teardown();
  }

  // ----- Public toggles -----

  open(srcEl) {
    if (!srcEl?.src) return;
    this.triggerEl = srcEl;
    if (!this.root) this._buildRoot();
    this.imgEl.src = srcEl.src;
    this.imgEl.alt = srcEl.alt || '';
    this.pathEl.textContent = srcEl.closest('.v2-embed-image')?.dataset.path || srcEl.alt || '';
    this._resetTransform();
    this.root.classList.add('open');
    this.root.setAttribute('aria-hidden', 'false');
  }

  close() {
    if (!this.root) return;
    this.root.classList.remove('open');
    this.root.setAttribute('aria-hidden', 'true');
    if (this.triggerEl?.focus) this.triggerEl.focus();
    this.triggerEl = null;
  }

  isOpen() { return !!this.root?.classList.contains('open'); }

  // ----- Build / teardown -----

  _buildRoot() {
    const host = document.createElement('div');
    host.className = 'v2-lightbox';
    host.setAttribute('role', 'dialog');
    host.setAttribute('aria-modal', 'true');
    host.setAttribute('aria-hidden', 'true');
    host.innerHTML = `
      <button class="v2-lightbox-close" type="button" title="Close" aria-label="Close">
        <svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>
      </button>
      <img class="v2-lightbox-img" alt="" draggable="false">
      <div class="v2-lightbox-path"></div>
    `;
    document.body.appendChild(host);
    this.root = host;
    this.imgEl = host.querySelector('.v2-lightbox-img');
    this.pathEl = host.querySelector('.v2-lightbox-path');

    host.addEventListener('click', this._onHostClick);
    host.addEventListener('wheel', this._onWheel, { passive: false });
    host.addEventListener('dblclick', this._onDblClick);
    host.addEventListener('pointerdown', this._onPointerDown);
    host.addEventListener('pointermove', this._onPointerMove);
    host.addEventListener('pointerup', this._onPointerUp);
    host.addEventListener('pointercancel', this._onPointerUp);
  }

  _teardown() {
    if (!this.root) return;
    this.root.remove();
    this.root = null;
    this.imgEl = null;
    this.pathEl = null;
  }

  // ----- Transform -----

  _apply() {
    if (!this.imgEl) return;
    this.imgEl.style.transform =
      `translate(${this.tx.toFixed(2)}px, ${this.ty.toFixed(2)}px) scale(${this.scale.toFixed(4)})`;
  }

  _resetTransform() {
    this.scale = 1;
    this.tx = 0;
    this.ty = 0;
    this._apply();
  }

  /** Zoom anchored at a point (cx, cy) in viewport coords.
   *  Derivation: viewport = image_center + (local * s) + (tx, ty).
   *  For the point under (cx, cy) to stay put after scale s0 → s1:
   *      tx1 = dx*(1 - k) + tx0*k,  where k = s1 / s0
   *  and dx = cx - imageCenter.x (same for dy). */
  _zoomAt(nextScale, cx, cy) {
    const s0 = this.scale;
    const s1 = clamp(nextScale, MIN_SCALE, MAX_SCALE);
    if (s1 === s0) return;
    const rect = this.imgEl.getBoundingClientRect();
    // `rect` already includes translate + scale, so its center is
    // image_center + (tx, ty). Back it out to get the unoffset center.
    const cxCenter = rect.left + rect.width / 2 - this.tx;
    const cyCenter = rect.top + rect.height / 2 - this.ty;
    const dx = cx - cxCenter;
    const dy = cy - cyCenter;
    const k = s1 / s0;
    this.tx = dx * (1 - k) + this.tx * k;
    this.ty = dy * (1 - k) + this.ty * k;
    this.scale = s1;
    this._apply();
  }

  // ----- Event handlers -----

  _onDocClick = (e) => {
    const img = e.target.closest?.('.v2-embed-image img');
    if (!img) return;
    e.preventDefault();
    this.open(img);
  };

  _onKey = (e) => {
    if (!this.isOpen()) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      this.close();
    }
  };

  _onHostClick = (e) => {
    if (e.target.closest('.v2-lightbox-close')) { this.close(); return; }
    if (e.target === this.root) this.close();  // backdrop
  };

  _onWheel = (e) => {
    e.preventDefault();
    const step = e.deltaY < 0 ? 1.15 : 1 / 1.15;
    this._zoomAt(this.scale * step, e.clientX, e.clientY);
  };

  _onDblClick = (e) => {
    e.preventDefault();
    const next = this.scale > 1.05 ? 1 : DBLCLICK_SCALE;
    this._zoomAt(next, e.clientX, e.clientY);
  };

  _onPointerDown = (e) => {
    if (!this.isOpen()) return;
    this.root.setPointerCapture?.(e.pointerId);
    this.pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (this.pointers.size === 2) {
      const pts = [...this.pointers.values()];
      this._pinchStartDist = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y) || 1;
      this._pinchStartScale = this.scale;
      this._panLast = null;
    } else if (this.pointers.size === 1 && this.scale > 1.02) {
      this._panLast = { x: e.clientX, y: e.clientY };
    }
  };

  _onPointerMove = (e) => {
    if (!this.pointers.has(e.pointerId)) return;
    this.pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });

    if (this.pointers.size >= 2) {
      const pts = [...this.pointers.values()];
      const [a, b] = pts;
      const dist = Math.hypot(a.x - b.x, a.y - b.y);
      const cx = (a.x + b.x) / 2;
      const cy = (a.y + b.y) / 2;
      const nextScale = this._pinchStartScale * (dist / this._pinchStartDist);
      this._zoomAt(nextScale, cx, cy);
      return;
    }

    if (this.pointers.size === 1 && this._panLast && this.scale > 1.02) {
      const dx = e.clientX - this._panLast.x;
      const dy = e.clientY - this._panLast.y;
      this._panLast = { x: e.clientX, y: e.clientY };
      this.tx += dx;
      this.ty += dy;
      this._apply();
    }
  };

  _onPointerUp = (e) => {
    this.pointers.delete(e.pointerId);
    if (this.pointers.size < 2) {
      this._pinchStartDist = 0;
      this._pinchStartScale = this.scale;
    }
    if (this.pointers.size === 0) {
      this._panLast = null;
    }
  };
}
