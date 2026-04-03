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

    // Auto-link bare URLs (skip those already inside <a> or <code> tags)
    text = text.replace(/((?:<a\b[^>]*>.*?<\/a>)|(?:<code\b[^>]*>.*?<\/code>))|(https?:\/\/[^\s<)]+)/g,
      function (match, tagged, url) {
        if (tagged) return tagged; // already wrapped — leave it
        // Trim trailing punctuation and HTML entities (&quot; &amp; etc.) that aren't part of the URL
        var clean = url.replace(/(?:&(?:quot|amp|lt|gt|#\d+|#x[\da-fA-F]+);[.,;:!?)]*|[.,;:!?)]+)+$/, '');
        var trailing = url.slice(clean.length);
        return '<a href="' + clean + '" target="_blank" rel="noopener" class="md-link">' + clean + '</a>' + trailing;
      });

    return text;
  }

  // ── file / diff embed helpers ────────────────────────────────────────────

  var buildFileRe = /<build-file\s+path="([^"]*)"(?:\s+lang="([^"]*)")?(?:\s+lines="(\d+-\d+)")?>\n([\s\S]*?)\n<\/build-file>/g;
  var buildDiffRe = /<build-diff\s+path="([^"]*)">\n([\s\S]*?)\n<\/build-diff>/g;

  function renderInlineFile(content, path, lang, lineRange) {
    var lines = content.split('\n');
    if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
    var totalLines = lines.length;

    var ext = lang || '';
    if (!ext) {
      var extMatch = path.match(/\.([^./]+)$/);
      ext = extMatch ? extMatch[1].toLowerCase() : '';
    }

    var lineStart = 1;
    var meta = '';
    if (lineRange) {
      var parts = lineRange.split('-');
      lineStart = parseInt(parts[0], 10) || 1;
      meta = '<span class="build-embed-meta">lines ' + escapeHtml(lineRange) + '</span>';
    }

    var collapsed = totalLines > 8;
    var cls = 'build-embed' + (collapsed ? ' collapsed' : '');
    var toggleBtn = collapsed
      ? '<button class="build-embed-toggle" data-show-text="Show ' + totalLines + ' lines">Show ' + totalLines + ' lines</button>'
      : '';

    var viewerHtml = '<div class="file-viewer">';
    for (var i = 0; i < lines.length; i++) {
      var hl = (typeof window !== 'undefined' && window.highlightLine) ? window.highlightLine(lines[i], ext) : escapeHtml(lines[i]);
      viewerHtml += '<div class="file-line"><span class="fl-num">' + (lineStart + i) + '</span><span class="fl-content">' + hl + '</span></div>';
    }
    viewerHtml += '</div>';

    return '<div class="' + cls + '" data-embed-type="file">' +
      '<div class="build-embed-header">' +
        '<span class="build-embed-path">' + escapeHtml(path) + '</span>' +
        meta + toggleBtn +
      '</div>' +
      '<div class="build-embed-body">' + viewerHtml + '</div>' +
    '</div>';
  }

  function renderInlineDiff(diffText, path) {
    var lines = diffText.split('\n');
    var totalLines = 0;

    var extMatch = path.match(/\.([^./]+)$/);
    var ext = extMatch ? extMatch[1].toLowerCase() : '';

    var viewerHtml = '<div class="diff-viewer">';
    var oldNum = 0, newNum = 0;
    for (var i = 0; i < lines.length; i++) {
      var line = lines[i];
      if (line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('---') || line.startsWith('+++')) continue;
      if (line.startsWith('@@')) {
        var m = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
        if (m) { oldNum = parseInt(m[1]); newNum = parseInt(m[2]); }
        viewerHtml += '<div class="diff-hunk-header">' + escapeHtml(line) + '</div>';
        totalLines++;
        continue;
      }
      var hl;
      var cls = 'diff-line';
      var numText;
      if (line.startsWith('+')) {
        cls += ' added';
        numText = String(newNum++);
        hl = (typeof window !== 'undefined' && window.highlightLine) ? window.highlightLine(line.slice(1), ext) : escapeHtml(line.slice(1));
      } else if (line.startsWith('-')) {
        cls += ' removed';
        numText = String(oldNum++);
        hl = (typeof window !== 'undefined' && window.highlightLine) ? window.highlightLine(line.slice(1), ext) : escapeHtml(line.slice(1));
      } else {
        cls += ' context';
        numText = String(newNum);
        oldNum++; newNum++;
        hl = (typeof window !== 'undefined' && window.highlightLine) ? window.highlightLine(line.startsWith(' ') ? line.slice(1) : line, ext) : escapeHtml(line);
      }
      viewerHtml += '<div class="' + cls + '"><span class="dl-num">' + numText + '</span><span class="dl-content">' + hl + '</span></div>';
      totalLines++;
    }
    viewerHtml += '</div>';

    var collapsed = totalLines > 8;
    var wrapCls = 'build-embed' + (collapsed ? ' collapsed' : '');
    var toggleBtn = collapsed
      ? '<button class="build-embed-toggle" data-show-text="Show ' + totalLines + ' lines">Show ' + totalLines + ' lines</button>'
      : '';

    return '<div class="' + wrapCls + '" data-embed-type="diff">' +
      '<div class="build-embed-header">' +
        '<span class="build-embed-path">' + escapeHtml(path) + '</span>' +
        toggleBtn +
      '</div>' +
      '<div class="build-embed-body">' + viewerHtml + '</div>' +
    '</div>';
  }

  // ── block parser ──────────────────────────────────────────────────────────

  function renderMarkdown(src) {
    if (!src) return '';

    // Extract <build-file> and <build-diff> blocks before markdown processing.
    var embeds = [];
    src = src.replace(buildFileRe, function (match, path, lang, lineRange, content) {
      var idx = embeds.length;
      embeds.push(renderInlineFile(content, path, lang || '', lineRange || ''));
      return '\x00BUILD_EMBED_' + idx + '\x00';
    });
    src = src.replace(buildDiffRe, function (match, path, content) {
      var idx = embeds.length;
      embeds.push(renderInlineDiff(content, path));
      return '\x00BUILD_EMBED_' + idx + '\x00';
    });

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
      if (/^(\*{3,}|-{3,}|_{3,})\s*$/.test(line)) {
        out.push('<hr class="md-hr">');
        i++;
        continue;
      }

      // ── heading ─────────────────────────────────────────────────────────
      var headingMatch = line.match(/^(#{1,6})\s+(.+)$/);
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
          tHtml += '<th class="md-align-' + a + '">' + inlineMarkdown(escapeHtml(cell)) + '</th>';
        });
        tHtml += '</tr></thead><tbody>';
        tRows.forEach(function (row) {
          tHtml += '<tr>';
          row.forEach(function (cell, ci) {
            var a = aligns[ci] || 'left';
            tHtml += '<td class="md-align-' + a + '">' + inlineMarkdown(escapeHtml(cell)) + '</td>';
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
             !lines[i].match(/^#{1,6}\s/) &&
             !lines[i].match(/^>\s?/) &&
             !lines[i].match(/^[\-\*\+]\s+/) &&
             !lines[i].match(/^\d+\.\s+/) &&
             !lines[i].match(/^(\*{3,}|-{3,}|_{3,})\s*$/) &&
             !(/^\|(.+)\|/.test(lines[i]) && i + 1 < lines.length && /^\|[\s\-:|]+\|/.test(lines[i + 1]))) {
        paraLines.push(lines[i]);
        i++;
      }
      out.push('<p>' + inlineMarkdown(escapeHtml(paraLines.join('\n'))) + '</p>');
    }

    var result = out.join('');

    // Restore embedded file/diff blocks.
    for (var ei = 0; ei < embeds.length; ei++) {
      // The placeholder may have been wrapped in a <p> tag by the paragraph parser.
      result = result.replace('<p>\x00BUILD_EMBED_' + ei + '\x00</p>', embeds[ei]);
      result = result.replace('\x00BUILD_EMBED_' + ei + '\x00', embeds[ei]);
    }

    return result;
  }

  // ── exports ───────────────────────────────────────────────────────────────
  global.renderMarkdown = renderMarkdown;

})(typeof window !== 'undefined' ? window : this);
