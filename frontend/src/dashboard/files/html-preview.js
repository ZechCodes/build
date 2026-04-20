import { state } from '../state.js';
import { fileContentBody } from './refs.js';
import { getE2EE } from '../e2ee/bridge.js';
import { readFileAsync } from './content.js';
import { selectFile } from './tree.js';

export const MIME_TYPES = {
  css: 'text/css', js: 'application/javascript', mjs: 'application/javascript',
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif',
  webp: 'image/webp', ico: 'image/x-icon', svg: 'image/svg+xml',
  woff: 'font/woff', woff2: 'font/woff2', ttf: 'font/ttf', eot: 'application/vnd.ms-fontobject',
  json: 'application/json',
};

export function resolveAssetPath(htmlFilePath, assetHref) {
  if (!assetHref || assetHref.startsWith('data:') || assetHref.startsWith('http:') || assetHref.startsWith('https:') || assetHref.startsWith('//')) return null;
  // Strip query/hash.
  const clean = assetHref.split('?')[0].split('#')[0];
  // Resolve relative to HTML file's directory.
  const dir = htmlFilePath.substring(0, htmlFilePath.lastIndexOf('/') + 1);
  // Simple path resolution (handles ../ and ./).
  const parts = (dir + clean).split('/');
  const resolved = [];
  for (const p of parts) {
    if (p === '.' || p === '') continue;
    if (p === '..') { resolved.pop(); continue; }
    resolved.push(p);
  }
  return resolved.join('/');
}

// Console capture JS (raw code, no <script> tags — injected via DOM).
export const CONSOLE_CAPTURE_JS = `(function(){
  function send(level, args) {
    var parts = [];
    for (var i = 0; i < args.length; i++) {
      try { parts.push(typeof args[i] === 'string' ? args[i] : JSON.stringify(args[i], null, 2)); }
      catch(e) { parts.push(String(args[i])); }
    }
    parent.postMessage({ type: '__build_console', entry: { level: level, text: parts.join(' '), ts: Date.now() } }, '*');
  }
  var orig = {};
  ['log','warn','error','info','debug'].forEach(function(m){
    orig[m] = console[m];
    console[m] = function(){ send(m, arguments); if(orig[m]) orig[m].apply(console, arguments); };
  });
  window.onerror = function(msg, src, line, col) {
    send('error', [msg + (src ? ' at ' + src + ':' + line + ':' + col : '')]);
  };
  window.addEventListener('unhandledrejection', function(e) {
    send('error', ['Unhandled rejection: ' + (e.reason && e.reason.message || e.reason || 'unknown')]);
  });
  window.addEventListener('error', function(e) {
    if (e.target && e.target !== window) {
      var tag = e.target.tagName || '';
      var src = e.target.src || e.target.href || '';
      send('error', ['Failed to load ' + tag.toLowerCase() + (src ? ': ' + src : '')]);
    }
  }, true);
})();`;

// Script to intercept relative link clicks and navigate via parent.
export const NAV_INTERCEPT_JS = `(function(){
  document.addEventListener('click', function(e) {
    var a = e.target.closest('a[href]');
    if (!a) return;
    var href = a.getAttribute('href');
    if (!href || href.startsWith('#') || href.startsWith('javascript:')) return;
    if (href.startsWith('http:') || href.startsWith('https:') || href.startsWith('//') || href.startsWith('mailto:')) return;
    e.preventDefault();
    parent.postMessage({ type: '__build_preview_navigate', href: href }, '*');
  });
})();`;

// ---- Browser Proxy URL fetch ----

export function urlFetchAsync(deviceId, url, tabId, method, body, contentType) {
  return new Promise((resolve, reject) => {
    const conn = state.e2eeConnections.get(deviceId);
    if (!conn || !conn.connected) return reject(new Error('not connected'));
    const requestId = 'rf-' + Date.now() + '-' + Math.random().toString(36).slice(2, 6);
    function handler(evt) {
      const d = evt.detail;
      if (d.request_id !== requestId) return;
      conn.removeEventListener('url_fetch_result', handler);
      if (d.error) return reject(new Error(d.error));
      resolve(d);
    }
    conn.addEventListener('url_fetch_result', handler);
    conn.urlFetch(url, requestId, tabId || '', method, body, contentType);
  });
}

export function resolveUrl(baseUrl, href) {
  if (!href || href.startsWith('data:') || href.startsWith('#') || href.startsWith('javascript:')) return null;
  try { return new URL(href, baseUrl).href; } catch { return null; }
}

// ---- HTML Preview with Asset Inlining ----

// Tracks asset-fetch errors to surface in the console overlay.
let _htmlPreviewErrors = [];

export async function renderHtmlPreview(content, htmlPath) {
  fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading preview...</p></div>';
  _htmlPreviewErrors = [];
  const _dbg = (text) => _htmlPreviewErrors.push({ level: 'debug', text });

  const channelId = state.filesChannelId;
  let html = content;

  _dbg('Rendering ' + htmlPath + ' (' + content.length + ' bytes)');

  const isExternal = (url) => url && (url.startsWith('http:') || url.startsWith('https:') || url.startsWith('//'));

  // Use string-based asset resolution (avoids DOMParser mangling style/script content).
  const replacements = [];

  // Find local CSS <link> tags (external left as-is — blob iframe has no CSP restrictions).
  const linkRe = /<link\b[^>]*\brel\s*=\s*["']stylesheet["'][^>]*>|<link\b[^>]*\bhref\s*=\s*["'][^"']+["'][^>]*\brel\s*=\s*["']stylesheet["'][^>]*>/gi;
  const hrefRe = /\bhref\s*=\s*["']([^"']+)["']/i;
  for (const m of html.matchAll(linkRe)) {
    const tag = m[0];
    const hm = tag.match(hrefRe);
    if (!hm) continue;
    const href = hm[1];
    if (isExternal(href)) { _dbg('CSS (ext, kept): ' + href); continue; }
    const resolved = resolveAssetPath(htmlPath, href);
    _dbg('CSS (local): ' + href + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.content && !result.is_binary && !result.is_image) {
        _dbg('CSS OK: ' + result.content.length + ' bytes');
        return { original: tag, replacement: '<style>' + result.content + '</style>' };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'CSS failed: ' + href + ' (' + err.message + ')' });
      return null;
    }));
  }

  // Find local script[src] tags (external left as-is).
  const scriptRe = /<script\b[^>]*\bsrc\s*=\s*["']([^"']+)["'][^>]*>\s*<\/script>/gi;
  for (const m of html.matchAll(scriptRe)) {
    const tag = m[0];
    const src = m[1];
    if (isExternal(src)) { _dbg('JS (ext, kept): ' + src); continue; }
    const resolved = resolveAssetPath(htmlPath, src);
    _dbg('JS (local): ' + src + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.content && !result.is_binary && !result.is_image) {
        _dbg('JS OK: ' + result.content.length + ' bytes');
        return { original: tag, replacement: '<script>' + result.content + '<\/script>' };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'JS failed: ' + src + ' (' + err.message + ')' });
      return null;
    }));
  }

  // Find local <img src> tags.
  const imgRe = /(<img\b[^>]*\bsrc\s*=\s*["'])([^"']+)(["'][^>]*>)/gi;
  for (const m of html.matchAll(imgRe)) {
    const tag = m[0];
    const src = m[2];
    if (isExternal(src) || src.startsWith('data:')) continue;
    const resolved = resolveAssetPath(htmlPath, src);
    _dbg('IMG (local): ' + src + ' → ' + resolved);
    if (!resolved) continue;
    replacements.push(readFileAsync(channelId, resolved).then(result => {
      if (result.is_image && result.content) {
        _dbg('IMG OK: ' + resolved);
        return { original: tag, replacement: m[1] + result.content + m[3] };
      } else if (result.content && !result.is_binary) {
        const ext = resolved.split('.').pop().toLowerCase();
        const mime = MIME_TYPES[ext] || 'application/octet-stream';
        return { original: tag, replacement: m[1] + 'data:' + mime + ';base64,' + btoa(result.content) + m[3] };
      }
      return null;
    }).catch(err => {
      _htmlPreviewErrors.push({ level: 'error', text: 'IMG failed: ' + src + ' (' + err.message + ')' });
      return null;
    }));
  }

  _dbg('Fetching ' + replacements.length + ' assets...');
  const results = await Promise.all(replacements);
  for (const r of results) {
    if (r) html = html.replace(r.original, r.replacement);
  }
  _dbg('Done. ' + _htmlPreviewErrors.filter(e => e.level === 'error').length + ' errors');

  // Bail if user navigated away.
  if (state.filesCurrentPath !== htmlPath || state.filesChannelId !== channelId) return;

  // Inject console capture script right after <head>.
  const headMatch = html.match(/<head[^>]*>/i);
  if (headMatch) {
    const idx = html.indexOf(headMatch[0]) + headMatch[0].length;
    html = html.slice(0, idx) + '<script>' + CONSOLE_CAPTURE_JS + NAV_INTERCEPT_JS + '<\/script>' + html.slice(idx);
  }

  const inlinedHtml = html;

  // Build wrapper with iframe and console overlay.
  fileContentBody.innerHTML = '';
  fileContentBody.style.padding = '0';

  const wrapper = document.createElement('div');
  wrapper.style.cssText = 'position:relative;width:100%;height:100%;display:flex;flex-direction:column';

  const iframe = document.createElement('iframe');
  // No sandbox attr — blob URL already has opaque origin (isolated from parent).
  iframe.style.cssText = 'width:100%;flex:1;border:none;background:#fff;border-radius:4px 4px 0 0;min-height:0';
  wrapper.appendChild(iframe);

  // Console overlay.
  const consoleBar = document.createElement('div');
  consoleBar.className = 'html-console-bar';
  consoleBar.innerHTML = '<span class="html-console-title">Console</span><span class="html-console-badge hc-hidden">0</span><span class="html-console-toggle">&#x25B2;</span>';
  wrapper.appendChild(consoleBar);

  const consolePanel = document.createElement('div');
  consolePanel.className = 'html-console-panel hc-hidden';
  wrapper.appendChild(consolePanel);

  fileContentBody.appendChild(wrapper);

  const badge = consoleBar.querySelector('.html-console-badge');
  const toggleIcon = consoleBar.querySelector('.html-console-toggle');
  let consoleOpen = false;
  let entryCount = 0;

  consoleBar.addEventListener('click', () => {
    consoleOpen = !consoleOpen;
    consolePanel.classList.toggle('hc-hidden', !consoleOpen);
    toggleIcon.innerHTML = consoleOpen ? '&#x25BC;' : '&#x25B2;';
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

  // Surface asset-fetch errors/debug that happened before iframe loaded.
  for (const err of _htmlPreviewErrors) addConsoleEntry(err.level, err.text);
  if (_htmlPreviewErrors.length > 0) {
    consoleOpen = true;
    consolePanel.classList.remove('hc-hidden');
    toggleIcon.innerHTML = '&#x25BC;';
    badge.classList.add('hc-hidden');
  }

  function onMsg(evt) {
    if (!evt.data) return;
    if (evt.data.type === '__build_console') {
      addConsoleEntry(evt.data.entry.level, evt.data.entry.text);
    } else if (evt.data.type === '__build_preview_ready') {
      iframe.contentWindow.postMessage({ type: '__build_preview', html: inlinedHtml }, '*');
    } else if (evt.data.type === '__build_preview_navigate') {
      const targetPath = resolveAssetPath(htmlPath, evt.data.href);
      if (targetPath) {
        const data = state.fileTreeData.get(state.filesChannelId);
        const dir = targetPath.substring(0, targetPath.lastIndexOf('/') + 1);
        const dirKey = dir ? dir.slice(0, -1) : '';
        const entries = data && data.get(dirKey);
        const entry = entries && entries.entries && entries.entries.find(e => (e.path || e.name) === targetPath);
        selectFile(targetPath, entry || null, 'rendered');
        const conn = getE2EE(state.filesChannelId);
        if (conn && conn.connected) {
          fileContentBody.innerHTML = '<div class="empty-state"><div class="loading-spinner"></div><p>Loading...</p></div>';
          conn.fileRead(state.filesChannelId, targetPath);
        }
      }
    }
  }
  window.addEventListener('message', onMsg);

  // Clean up listener when content changes.
  const observer = new MutationObserver(() => {
    if (!fileContentBody.contains(wrapper)) {
      window.removeEventListener('message', onMsg);
      observer.disconnect();
    }
  });
  observer.observe(fileContentBody, { childList: true });

  // Load preview frame (has its own permissive CSP).
  iframe.src = '/preview-frame';
}
