/**
 * Lightweight markdown → HTML renderer for Build chat messages.
 *
 * Supported syntax:
 *   **bold**  __bold__
 *   *italic*  _italic_
 *   ~~strikethrough~~
 *   `inline code`
 *   ```lang\n...\n```  (fenced code blocks)
 *   # / ## / ### headings
 *   - / * / + unordered lists
 *   1. ordered lists
 *   [text](url) links
 *   > blockquotes
 *   | col | col | tables (with alignment)
 *   --- / *** horizontal rules
 *   blank-line paragraph breaks
 *
 * HTML in source text is always escaped.
 */
(function (global) {
  'use strict';

  // ── helpers ────────────────────────────────────────────────────────────────

  function escapeHtml(str) {
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  // Process inline markdown on an already-escaped string.
  function inlineMarkdown(text) {
    // Inline code (must come first so backtick content is protected)
    text = text.replace(/`([^`]+?)`/g, '<code class="md-inline-code">$1</code>');

    // Images (before links so ![...](...) isn't caught by link regex)
    text = text.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, '<img alt="$1" src="$2" class="md-img">');

    // Links
    text = text.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener" class="md-link">$1</a>');

    // Bold (**text** or __text__)
    text = text.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
    text = text.replace(/__(.+?)__/g, '<strong>$1</strong>');

    // Italic (*text* or _text_) — avoid matching inside words for underscores
    text = text.replace(/\*(.+?)\*/g, '<em>$1</em>');
    text = text.replace(/(?<!\w)_(.+?)_(?!\w)/g, '<em>$1</em>');

    // Strikethrough
    text = text.replace(/~~(.+?)~~/g, '<del>$1</del>');

    return text;
  }

  // ── block parser ──────────────────────────────────────────────────────────

  function renderMarkdown(src) {
    if (!src) return '';

    var lines = src.split('\n');
    var out = [];
    var i = 0;

    while (i < lines.length) {
      var line = lines[i];

      // ── fenced code block ───────────────────────────────────────────────
      var fenceMatch = line.match(/^```(\w*)\s*$/);
      if (fenceMatch) {
        var lang = fenceMatch[1] || '';
        var codeLines = [];
        i++;
        while (i < lines.length && !lines[i].match(/^```\s*$/)) {
          codeLines.push(lines[i]);
          i++;
        }
        i++; // skip closing ```
        var langAttr = lang ? ' data-lang="' + escapeHtml(lang) + '"' : '';
        var langLabel = lang ? '<span class="md-code-lang">' + escapeHtml(lang) + '</span>' : '';
        out.push(
          '<div class="md-code-block"' + langAttr + '>' +
          langLabel +
          '<pre><code>' + escapeHtml(codeLines.join('\n')) + '</code></pre></div>'
        );
        continue;
      }

      // ── horizontal rule ─────────────────────────────────────────────────
      if (/^(\*\*\*|---|___)\s*$/.test(line)) {
        out.push('<hr class="md-hr">');
        i++;
        continue;
      }

      // ── heading ─────────────────────────────────────────────────────────
      var headingMatch = line.match(/^(#{1,3})\s+(.+)$/);
      if (headingMatch) {
        var level = headingMatch[1].length;
        out.push('<h' + level + ' class="md-h' + level + '">' + inlineMarkdown(escapeHtml(headingMatch[2])) + '</h' + level + '>');
        i++;
        continue;
      }

      // ── blockquote ──────────────────────────────────────────────────────
      if (/^>\s?/.test(line)) {
        var quoteLines = [];
        while (i < lines.length && /^>\s?/.test(lines[i])) {
          quoteLines.push(lines[i].replace(/^>\s?/, ''));
          i++;
        }
        out.push('<blockquote class="md-blockquote">' + renderMarkdown(quoteLines.join('\n')) + '</blockquote>');
        continue;
      }

      // ── unordered list ──────────────────────────────────────────────────
      if (/^[\-\*\+]\s+/.test(line)) {
        var items = [];
        while (i < lines.length && /^[\-\*\+]\s+/.test(lines[i])) {
          items.push(lines[i].replace(/^[\-\*\+]\s+/, ''));
          i++;
        }
        out.push('<ul class="md-list">' +
          items.map(function (item) { return '<li>' + inlineMarkdown(escapeHtml(item)) + '</li>'; }).join('') +
          '</ul>');
        continue;
      }

      // ── ordered list ────────────────────────────────────────────────────
      if (/^\d+\.\s+/.test(line)) {
        var olItems = [];
        while (i < lines.length && /^\d+\.\s+/.test(lines[i])) {
          olItems.push(lines[i].replace(/^\d+\.\s+/, ''));
          i++;
        }
        out.push('<ol class="md-list">' +
          olItems.map(function (item) { return '<li>' + inlineMarkdown(escapeHtml(item)) + '</li>'; }).join('') +
          '</ol>');
        continue;
      }

      // ── table ────────────────────────────────────────────────────────
      if (/^\|(.+)\|/.test(line) && i + 1 < lines.length && /^\|[\s\-:|]+\|/.test(lines[i + 1])) {
        var headerCells = line.split('|').slice(1, -1).map(function (c) { return c.trim(); });
        i++;
        var aligns = lines[i].split('|').slice(1, -1).map(function (c) {
          c = c.trim();
          if (c[0] === ':' && c[c.length - 1] === ':') return 'center';
          if (c[c.length - 1] === ':') return 'right';
          return 'left';
        });
        i++;
        var tRows = [];
        while (i < lines.length && /^\|(.+)\|/.test(lines[i])) {
          tRows.push(lines[i].split('|').slice(1, -1).map(function (c) { return c.trim(); }));
          i++;
        }
        var tHtml = '<div class="md-table-wrap"><table class="md-table"><thead><tr>';
        headerCells.forEach(function (cell, ci) {
          var a = aligns[ci] || 'left';
          tHtml += '<th style="text-align:' + a + '">' + inlineMarkdown(escapeHtml(cell)) + '</th>';
        });
        tHtml += '</tr></thead><tbody>';
        tRows.forEach(function (row) {
          tHtml += '<tr>';
          row.forEach(function (cell, ci) {
            var a = aligns[ci] || 'left';
            tHtml += '<td style="text-align:' + a + '">' + inlineMarkdown(escapeHtml(cell)) + '</td>';
          });
          tHtml += '</tr>';
        });
        tHtml += '</tbody></table></div>';
        out.push(tHtml);
        continue;
      }

    // ── blank line ──────────────────────────────────────────────────────
      if (/^\s*$/.test(line)) {
        i++;
        continue;
      }

      // ── paragraph (collect consecutive non-empty lines) ─────────────────
      var paraLines = [];
      while (i < lines.length && !/^\s*$/.test(lines[i]) &&
             !lines[i].match(/^```/) &&
             !lines[i].match(/^#{1,3}\s/) &&
             !lines[i].match(/^>\s?/) &&
             !lines[i].match(/^[\-\*\+]\s+/) &&
             !lines[i].match(/^\d+\.\s+/) &&
             !lines[i].match(/^(\*\*\*|---|___)\s*$/) &&
             !(/^\|(.+)\|/.test(lines[i]) && i + 1 < lines.length && /^\|[\s\-:|]+\|/.test(lines[i + 1]))) {
        paraLines.push(lines[i]);
        i++;
      }
      out.push('<p>' + inlineMarkdown(escapeHtml(paraLines.join('\n'))) + '</p>');
    }

    return out.join('');
  }

  // ── exports ───────────────────────────────────────────────────────────────
  global.renderMarkdown = renderMarkdown;

})(typeof window !== 'undefined' ? window : this);
