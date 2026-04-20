import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { renderChannelPanel } from '../channels/panel.js';
import { urlFetchAsync, resolveUrl, CONSOLE_CAPTURE_JS } from './html-preview.js';

async function fetchAndRenderBrowserPage(tab) {
  const browserContent = document.getElementById('browser-content');
  if (!browserContent) return;
  browserContent.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';

  const deviceId = tab.deviceId;
  const url = tab.url;
  let errors = [];
  const _dbg = (text) => errors.push({ level: 'debug', text });

  try {
    const method = tab._method || 'GET';
    const body = tab._body || undefined;
    const contentType = tab._contentType || undefined;
    // Clear one-shot POST data after use.
    tab._method = undefined;
    tab._body = undefined;
    tab._contentType = undefined;
    _dbg('Fetching ' + method + ' ' + url);
    const result = await urlFetchAsync(deviceId, url, tab.id, method, body, contentType);
    // Update URL bar if server redirected us.
    if (result.final_url && result.final_url !== url) {
      tab.url = result.final_url;
      const urlInput = document.getElementById('browser-url-input');
      if (urlInput) urlInput.value = result.final_url;
      _dbg('Redirected to ' + result.final_url);
    }
    if (result.is_binary) {
      browserContent.innerHTML = '<div class="empty-state"><p>Cannot display binary content</p></div>';
      return;
    }
    if (result.status && result.status >= 400) {
      _dbg('HTTP ' + result.status);
    }

    let html = result.content;
    const baseUrl = result.final_url || url;
    _dbg('Got ' + html.length + ' bytes');

    // Resolve and inline local assets (CSS, JS, images).
    const replacements = [];
    const isExternal = (u) => {
      if (!u) return true;
      try { const p = new URL(u, baseUrl); return p.origin !== new URL(baseUrl).origin; } catch { return true; }
    };

    // CSS links.
    const linkRe = /<link\b[^>]*\brel\s*=\s*["']stylesheet["'][^>]*>|<link\b[^>]*\bhref\s*=\s*["'][^"']+["'][^>]*\brel\s*=\s*["']stylesheet["'][^>]*>/gi;
    const hrefRe = /\bhref\s*=\s*["']([^"']+)["']/i;
    for (const m of html.matchAll(linkRe)) {
      const tag = m[0];
      const hm = tag.match(hrefRe);
      if (!hm) continue;
      const href = hm[1];
      const resolved = resolveUrl(baseUrl, href);
      if (!resolved || isExternal(resolved)) { _dbg('CSS (ext): ' + href); continue; }
      _dbg('CSS: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.content && !r.is_binary) {
          // Resolve url() references inside CSS.
          let css = r.content;
          css = css.replace(/url\(\s*["']?(?!data:|https?:|\/\/)([^"')]+)["']?\s*\)/g, (match, ref) => {
            const absRef = resolveUrl(resolved, ref);
            return absRef ? `url(${absRef})` : match;
          });
          return { original: tag, replacement: '<style>' + css + '</style>' };
        }
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'CSS failed: ' + href + ' (' + err.message + ')' }); return null; }));
    }

    // Script tags.
    const scriptRe = /<script\b[^>]*\bsrc\s*=\s*["']([^"']+)["'][^>]*>\s*<\/script>/gi;
    for (const m of html.matchAll(scriptRe)) {
      const tag = m[0];
      const src = m[1];
      const resolved = resolveUrl(baseUrl, src);
      if (!resolved || isExternal(resolved)) { _dbg('JS (ext): ' + src); continue; }
      _dbg('JS: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.content && !r.is_binary) return { original: tag, replacement: '<script>' + r.content + '<\/script>' };
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'JS failed: ' + src + ' (' + err.message + ')' }); return null; }));
    }

    // Images.
    const imgRe = /(<img\b[^>]*\bsrc\s*=\s*["'])([^"']+)(["'][^>]*>)/gi;
    for (const m of html.matchAll(imgRe)) {
      const tag = m[0];
      const src = m[2];
      if (src.startsWith('data:')) continue;
      const resolved = resolveUrl(baseUrl, src);
      if (!resolved || isExternal(resolved)) continue;
      _dbg('IMG: ' + resolved);
      replacements.push(urlFetchAsync(deviceId, resolved, tab.id).then(r => {
        if (r.is_binary && r.content) {
          const ct = r.content_type || 'image/png';
          const mime = ct.split(';')[0].trim();
          return { original: tag, replacement: m[1] + 'data:' + mime + ';base64,' + r.content + m[3] };
        }
        return null;
      }).catch(err => { errors.push({ level: 'error', text: 'IMG failed: ' + src + ' (' + err.message + ')' }); return null; }));
    }

    _dbg('Fetching ' + replacements.length + ' assets...');
    const results = await Promise.all(replacements);
    for (const r of results) {
      if (r) html = html.replace(r.original, r.replacement);
    }

    // Bail if user navigated away.
    if (state.activeBrowserTab !== tab.id) return;

    // Inject console capture + navigation intercept.
    const headMatch = html.match(/<head[^>]*>/i);
    if (headMatch) {
      const idx = html.indexOf(headMatch[0]) + headMatch[0].length;
      const navJs = `(function(){
        // --- Patch fetch to proxy through Build ---
        var _origFetch = window.fetch;
        var _reqId = 0;
        var _pending = {};
        window.addEventListener('message', function(evt) {
          if (evt.data && evt.data.type === '__build_fetch_response' && _pending[evt.data.reqId]) {
            _pending[evt.data.reqId](evt.data);
            delete _pending[evt.data.reqId];
          }
        });
        window.fetch = function(input, init) {
          init = init || {};
          var url = typeof input === 'string' ? input : (input && input.url ? input.url : String(input));
          var method = (init.method || (input && input.method) || 'GET').toUpperCase();
          var body = init.body || null;
          var contentType = null;
          var headers = init.headers;
          if (headers) {
            if (typeof headers.get === 'function') contentType = headers.get('content-type');
            else if (headers['Content-Type']) contentType = headers['Content-Type'];
            else if (headers['content-type']) contentType = headers['content-type'];
          }
          if (body && typeof body !== 'string') {
            try { body = new URLSearchParams(body).toString(); if (!contentType) contentType = 'application/x-www-form-urlencoded'; } catch(e) { body = String(body); }
          }
          var id = '__bf_' + (++_reqId);
          return new Promise(function(resolve) {
            _pending[id] = function(data) {
              var respInit = { status: data.status || 200, headers: { 'Content-Type': data.contentType || 'text/plain' } };
              resolve(new Response(data.body || '', respInit));
            };
            parent.postMessage({ type: '__build_browser_fetch', reqId: id, url: url, method: method, body: body, contentType: contentType }, '*');
          });
        };

        // --- Patch XMLHttpRequest to proxy through Build ---
        var _OrigXHR = XMLHttpRequest;
        function ProxyXHR() {
          this._method = 'GET'; this._url = ''; this._headers = {}; this._async = true;
          this.readyState = 0; this.status = 0; this.statusText = '';
          this.responseText = ''; this.response = ''; this.responseType = '';
          this.onreadystatechange = null; this.onload = null; this.onerror = null;
          this._listeners = {};
        }
        ProxyXHR.prototype.open = function(method, url, async) { this._method = method; this._url = url; this._async = async !== false; this.readyState = 1; };
        ProxyXHR.prototype.setRequestHeader = function(k, v) { this._headers[k.toLowerCase()] = v; };
        ProxyXHR.prototype.getResponseHeader = function(k) { return this._responseHeaders ? (this._responseHeaders[k.toLowerCase()] || null) : null; };
        ProxyXHR.prototype.getAllResponseHeaders = function() { return ''; };
        ProxyXHR.prototype.addEventListener = function(e, fn) { if (!this._listeners[e]) this._listeners[e] = []; this._listeners[e].push(fn); };
        ProxyXHR.prototype.removeEventListener = function(e, fn) { if (this._listeners[e]) this._listeners[e] = this._listeners[e].filter(function(f){return f !== fn;}); };
        ProxyXHR.prototype._fire = function(e) { var fns = this._listeners[e] || []; for (var i = 0; i < fns.length; i++) fns[i].call(this, {}); };
        ProxyXHR.prototype.send = function(body) {
          var self = this;
          var id = '__bf_' + (++_reqId);
          _pending[id] = function(data) {
            self.status = data.status || 200;
            self.statusText = data.status ? String(data.status) : 'OK';
            self.responseText = data.body || '';
            self.response = data.body || '';
            self._responseHeaders = { 'content-type': data.contentType || 'text/plain' };
            self.readyState = 4;
            if (self.onreadystatechange) self.onreadystatechange();
            if (self.onload) self.onload();
            self._fire('readystatechange');
            self._fire('load');
            self._fire('loadend');
          };
          parent.postMessage({ type: '__build_browser_fetch', reqId: id, url: self._url, method: self._method, body: body || null, contentType: self._headers['content-type'] || null }, '*');
        };
        ProxyXHR.prototype.abort = function() {};
        ProxyXHR.prototype.overrideMimeType = function() {};
        window.XMLHttpRequest = ProxyXHR;

        // --- Navigation intercept (lower priority: skip if page already handled) ---
        document.addEventListener('click', function(e) {
          if (e.defaultPrevented) return;
          var a = e.target.closest('a[href]');
          if (!a) return;
          var href = a.getAttribute('href');
          if (!href || href.startsWith('#') || href.startsWith('javascript:')) return;
          e.preventDefault();
          parent.postMessage({ type: '__build_browser_navigate', href: href }, '*');
        });
        document.addEventListener('submit', function(e) {
          if (e.defaultPrevented) return;
          var form = e.target;
          if (!form || form.tagName !== 'FORM') return;
          e.preventDefault();
          var action = form.getAttribute('action') || window.location.href;
          var method = (form.getAttribute('method') || 'GET').toUpperCase();
          var fd = new FormData(form);
          if (method === 'GET') {
            var params = new URLSearchParams(fd).toString();
            var sep = action.indexOf('?') === -1 ? '?' : '&';
            parent.postMessage({ type: '__build_browser_navigate', href: action + sep + params }, '*');
          } else {
            parent.postMessage({ type: '__build_browser_form_submit', action: action, method: method, body: new URLSearchParams(fd).toString(), contentType: 'application/x-www-form-urlencoded' }, '*');
          }
        });
      })();`;
      html = html.slice(0, idx) + '<script>' + CONSOLE_CAPTURE_JS + navJs + '<\/script>' + html.slice(idx);
    }

    const inlinedHtml = html;

    // Build wrapper.
    browserContent.innerHTML = '';
    const wrapper = document.createElement('div');
    wrapper.style.cssText = 'position:relative;width:100%;height:100%;display:flex;flex-direction:column';

    const iframe = document.createElement('iframe');
    iframe.style.cssText = 'width:100%;flex:1;border:none;background:#fff;min-height:0';
    wrapper.appendChild(iframe);

    // Console overlay (matches activity/terminal console style).
    const consoleBar = document.createElement('div');
    consoleBar.className = 'html-console-bar';
    consoleBar.innerHTML = '<span class="html-console-title">Console</span><span class="html-console-badge hc-hidden">0</span><span class="html-console-toggle"><svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 9l4-4 4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg></span>';
    wrapper.appendChild(consoleBar);

    const consolePanel = document.createElement('div');
    consolePanel.className = 'html-console-panel hc-hidden';
    wrapper.appendChild(consolePanel);

    browserContent.appendChild(wrapper);

    const badge = consoleBar.querySelector('.html-console-badge');
    const toggleIcon = consoleBar.querySelector('.html-console-toggle');
    let consoleOpen = false;
    let entryCount = 0;

    consoleBar.addEventListener('click', () => {
      consoleOpen = !consoleOpen;
      consolePanel.classList.toggle('hc-hidden', !consoleOpen);
      toggleIcon.innerHTML = consoleOpen
        ? '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 5l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>'
        : '<svg viewBox="0 0 14 14" fill="none" width="14" height="14"><path d="M3 9l4-4 4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';
      if (consoleOpen) { badge.classList.add('hc-hidden'); consolePanel.scrollTop = consolePanel.scrollHeight; }
    });

    function addConsoleEntry(level, text) {
      entryCount++;
      if (!consoleOpen) { badge.classList.remove('hc-hidden'); badge.textContent = String(entryCount); }
      const row = document.createElement('div');
      row.className = 'html-console-entry html-console-' + (level || 'log');
      const levelSpan = document.createElement('span');
      levelSpan.className = 'html-console-level';
      levelSpan.textContent = level || 'log';
      row.appendChild(levelSpan);
      const textSpan = document.createElement('span');
      textSpan.textContent = text;
      row.appendChild(textSpan);
      consolePanel.appendChild(row);
      if (consoleOpen) consolePanel.scrollTop = consolePanel.scrollHeight;
    }

    // Populate errors but keep console collapsed by default.
    for (const err of errors) addConsoleEntry(err.level, err.text);

    function onMsg(evt) {
      if (!evt.data) return;
      if (evt.data.type === '__build_console') {
        addConsoleEntry(evt.data.entry.level, evt.data.entry.text);
      } else if (evt.data.type === '__build_preview_ready') {
        iframe.contentWindow.postMessage({ type: '__build_preview', html: inlinedHtml }, '*');
      } else if (evt.data.type === '__build_browser_navigate') {
        const target = resolveUrl(baseUrl, evt.data.href);
        if (target && new URL(target).origin === new URL(baseUrl).origin) {
          tab.url = target;
          tab._method = undefined;
          tab._body = undefined;
          tab._contentType = undefined;
          document.getElementById('browser-url-input').value = target;
          renderChannelPanel();
          fetchAndRenderBrowserPage(tab);
        }
      } else if (evt.data.type === '__build_browser_form_submit') {
        const target = resolveUrl(baseUrl, evt.data.action);
        if (target && new URL(target).origin === new URL(baseUrl).origin) {
          tab.url = target;
          tab._method = evt.data.method;
          tab._body = evt.data.body;
          tab._contentType = evt.data.contentType;
          document.getElementById('browser-url-input').value = target;
          renderChannelPanel();
          fetchAndRenderBrowserPage(tab);
        }
      } else if (evt.data.type === '__build_browser_fetch') {
        const reqId = evt.data.reqId;
        const fetchUrl = resolveUrl(baseUrl, evt.data.url);
        if (!fetchUrl) {
          iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: 0, body: 'Invalid URL', contentType: 'text/plain' }, '*');
          return;
        }
        urlFetchAsync(deviceId, fetchUrl, tab.id, evt.data.method || 'GET', evt.data.body || undefined, evt.data.contentType || undefined)
          .then(r => {
            iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: r.status || 200, body: r.content || '', contentType: r.content_type || 'text/plain' }, '*');
          })
          .catch(err => {
            iframe.contentWindow.postMessage({ type: '__build_fetch_response', reqId, status: 0, body: err.message, contentType: 'text/plain' }, '*');
          });
      }
    }
    window.addEventListener('message', onMsg);

    const observer = new MutationObserver(() => {
      if (!browserContent.contains(wrapper)) {
        window.removeEventListener('message', onMsg);
        observer.disconnect();
      }
    });
    observer.observe(browserContent, { childList: true });

    iframe.src = '/preview-frame';

  } catch (err) {
    browserContent.innerHTML = '<div class="empty-state"><p>Error: ' + escapeHtml(err.message) + '</p></div>';
  }
}

export function showBrowserView(tab) {
  // Hide all tab panels, show browser panel.
  document.querySelectorAll('.tab-panel').forEach(p => p.classList.remove('active'));
  document.getElementById('tab-browser')?.classList.add('active');
  // Hide tab buttons active state.
  document.querySelectorAll('.viewer-tab[data-tab]').forEach(btn => btn.classList.remove('active'));
  // Browser owns the full right column — hide everything else.
  document.getElementById('viewer-tabs')?.classList.add('hidden');
  document.querySelector('.viewer-body')?.classList.add('hidden');
  document.querySelector('.comp-wrapper')?.classList.add('hidden');
  document.getElementById('console-bottom')?.classList.add('hidden');

  const urlInput = document.getElementById('browser-url-input');
  if (urlInput) urlInput.value = tab.url || '';

  // If URL looks valid, load it.
  if (tab.url && tab.url.startsWith('http')) {
    fetchAndRenderBrowserPage(tab);
  } else {
    const browserContent = document.getElementById('browser-content');
    if (browserContent) browserContent.innerHTML = '<div class="empty-state"><p>Enter a localhost address and press Go</p></div>';
  }
}

// Browser URL bar handlers.
document.getElementById('browser-url-go')?.addEventListener('click', () => {
  if (!state.activeBrowserTab) return;
  const urlInput = document.getElementById('browser-url-input');
  const url = urlInput?.value?.trim();
  if (!url) return;
  // Find the active tab and update its URL.
  for (const [, tabs] of state.browserTabs) {
    const tab = tabs.find(t => t.id === state.activeBrowserTab);
    if (tab) {
      tab.url = url;
      renderChannelPanel();
      fetchAndRenderBrowserPage(tab);
      break;
    }
  }
});

document.getElementById('browser-url-input')?.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    e.preventDefault();
    document.getElementById('browser-url-go')?.click();
  }
});
