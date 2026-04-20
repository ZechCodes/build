import { highlightLine } from './syntax.js';

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

    // Links — detect file paths vs URLs
    text = text.replace(/\[([^\]]+)\]\(([^)]+)\)/g, function (m, linkText, url) {
      if (/^https?:\/\/|^mailto:/i.test(url)) {
        return '<a href="' + url + '" target="_blank" rel="noopener" class="md-link">' + linkText + '</a>';
      }
      // Treat non-URL targets as file paths
      var pathOnly = url.replace(/:\d+(?::\d+)?$/, '');
      return '<a href="#" class="md-link file-path-link" data-file-path="' + pathOnly + '" title="' + pathOnly + '">' + linkText + '</a>';
    });

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
        var clean = url.replace(/(?:&(?:quot|amp|lt|gt|#\d+|#x[\da-fA-F]+);|[.,;:!?)])+$/, '');
        var trailing = url.slice(clean.length);
        return '<a href="' + clean + '" target="_blank" rel="noopener" class="md-link">' + clean + '</a>' + trailing;
      });

    // Auto-link file paths (skip already-tagged content)
    var _knownExts = '(?:js|jsx|ts|tsx|mjs|cjs|py|pyi|rb|go|rs|java|c|cc|cpp|h|hpp|cs|swift|kt|sh|bash|zsh|yml|yaml|json|jsonc|toml|ini|cfg|conf|xml|html|htm|css|scss|sass|less|md|markdown|txt|sql|graphql|gql|proto|vue|svelte|astro|prisma|tf|lock|gradle|cmake)';
    var _filePathRe = new RegExp(
      '((?:<a\\b[^>]*>[\\s\\S]*?<\\/a>)|(?:<code\\b[^>]*>[\\s\\S]*?<\\/code>))|' +
      '(' +
        '(?:' +
          // Paths starting with / ./ or ../
          '(?:\\.\\.\\/|\\.\\/)[\\/\\w.@-]+\\.' + _knownExts +
          '|' +
          '\\/[\\w.@-]+(?:\\/[\\w.@-]+)+\\.' + _knownExts +
          '|' +
          // Multi-segment relative paths (must contain /)
          '[\\w.@-]+\\/[\\w.@\\/-]*\\.' + _knownExts +
        ')' +
        '(?::\\d+(?::\\d+)?)?' +  // optional :line:col
        '|' +
        // Bare filename requires :line suffix as signal
        '[\\w@-]+\\.' + _knownExts + ':\\d+(?::\\d+)?' +
      ')', 'g');
    text = text.replace(_filePathRe, function (match, tagged, filePath) {
      if (tagged) return tagged;
      var pathOnly = filePath.replace(/:\d+(?::\d+)?$/, '');
      var lineMatch = filePath.match(/:(\d+)(?::(\d+))?$/);
      var lineAttr = lineMatch ? ' data-file-line="' + lineMatch[1] + '"' : '';
      return '<a href="#" class="md-link file-path-link" data-file-path="' + pathOnly + '"' + lineAttr + ' title="' + pathOnly + '">' + filePath + '</a>';
    });

    return text;
  }

  // ── word-level diff highlighting ─────────────────────────────────────────

  /**
   * Compare two strings and return HTML with the changed segments wrapped
   * in <span class="diff-word-{del|ins}">. Uses common-prefix/suffix
   * matching — simple but effective for typical single-line edits.
   */
  function wordDiffLine(oldStr, newStr) {
    // Find common prefix length.
    var maxPre = Math.min(oldStr.length, newStr.length);
    var pre = 0;
    while (pre < maxPre && oldStr[pre] === newStr[pre]) pre++;

    // Find common suffix length (not overlapping with prefix).
    var maxSuf = Math.min(oldStr.length - pre, newStr.length - pre);
    var suf = 0;
    while (suf < maxSuf && oldStr[oldStr.length - 1 - suf] === newStr[newStr.length - 1 - suf]) suf++;

    var oldMid = oldStr.substring(pre, oldStr.length - suf);
    var newMid = newStr.substring(pre, newStr.length - suf);
    var prefix = oldStr.substring(0, pre);
    var suffix = oldStr.substring(oldStr.length - suf);

    return {
      oldHtml: escapeHtml(prefix) +
        (oldMid ? '<span class="diff-word-del">' + escapeHtml(oldMid) + '</span>' : '') +
        escapeHtml(suffix),
      newHtml: escapeHtml(prefix) +
        (newMid ? '<span class="diff-word-ins">' + escapeHtml(newMid) + '</span>' : '') +
        escapeHtml(suffix),
    };
  }

  // Expose for the dashboard file-viewer diff renderer.

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
    if (lineRange) {
      var parts = lineRange.split('-');
      lineStart = parseInt(parts[0], 10) || 1;
    }

    var collapsed = totalLines > 8;
    var cls = 'build-embed' + (collapsed ? ' collapsed' : '');
    var toggleBtn = '<span class="build-embed-chevron">&#x25B6;</span>';

    var viewerHtml = '<div class="file-viewer">';
    for (var i = 0; i < lines.length; i++) {
      var hl = highlightLine(lines[i], ext);
      viewerHtml += '<div class="file-line"><span class="fl-num">' + (lineStart + i) + '</span><span class="fl-content">' + hl + '</span></div>';
    }
    viewerHtml += '</div>';

    var embedWrapBtn = '<button class="build-embed-wrap-toggle" title="Toggle line wrapping"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4h10M3 8h7a2 2 0 010 4H8l1.5-1.5M3 12h3"/></svg>wrap</button>';
    return '<div class="' + cls + '" data-embed-type="file">' +
      '<div class="build-embed-header">' +
        toggleBtn +
        '<span class="build-embed-path">' + escapeHtml(path) + '</span>' +
        embedWrapBtn +
      '</div>' +
      '<div class="build-embed-body">' + viewerHtml + '</div>' +
    '</div>';
  }

  function renderInlineDiff(diffText, path) {
    var lines = diffText.split('\n');
    var totalLines = 0;

    var extMatch = path.match(/\.([^./]+)$/);
    var ext = extMatch ? extMatch[1].toLowerCase() : '';

    // Pre-parse lines into typed entries, skipping diff headers.
    var entries = [];
    var tmpOldNum = 0, tmpNewNum = 0;
    for (var i = 0; i < lines.length; i++) {
      var line = lines[i];
      if (line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('---') || line.startsWith('+++')) continue;
      if (line.startsWith('@@')) {
        var m = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
        if (m) { tmpOldNum = parseInt(m[1]); tmpNewNum = parseInt(m[2]); }
        entries.push({ type: 'hunk', text: line });
        continue;
      }
      if (line.startsWith('+')) {
        entries.push({ type: 'add', text: line.slice(1), num: String(tmpNewNum++) });
      } else if (line.startsWith('-')) {
        entries.push({ type: 'del', text: line.slice(1), num: String(tmpOldNum++) });
      } else {
        entries.push({ type: 'ctx', text: line.startsWith(' ') ? line.slice(1) : line, num: String(tmpNewNum) });
        tmpOldNum++; tmpNewNum++;
      }
    }

    var viewerHtml = '<div class="diff-viewer">';
    var canHL = true;

    for (var ei = 0; ei < entries.length; ei++) {
      var e = entries[ei];
      if (e.type === 'hunk') {
        viewerHtml += '<div class="diff-hunk-header">' + escapeHtml(e.text) + '</div>';
        totalLines++;
        continue;
      }
      if (e.type === 'ctx') {
        var hl = canHL ? highlightLine(e.text, ext) : escapeHtml(e.text);
        viewerHtml += '<div class="diff-line context"><span class="dl-num">' + e.num + '</span><span class="dl-content">' + hl + '</span></div>';
        totalLines++;
        continue;
      }

      // Collect adjacent del/add runs for word-level diff.
      if (e.type === 'del') {
        var dels = [e];
        while (ei + 1 < entries.length && entries[ei + 1].type === 'del') dels.push(entries[++ei]);
        var adds = [];
        while (ei + 1 < entries.length && entries[ei + 1].type === 'add') adds.push(entries[++ei]);

        // Pair up del/add lines for word-level highlighting.
        var pairs = Math.min(dels.length, adds.length);
        for (var pi = 0; pi < dels.length; pi++) {
          var dHl;
          if (pi < pairs) {
            var wd = wordDiffLine(dels[pi].text, adds[pi].text);
            dHl = wd.oldHtml;
          } else {
            dHl = canHL ? highlightLine(dels[pi].text, ext) : escapeHtml(dels[pi].text);
          }
          viewerHtml += '<div class="diff-line removed"><span class="dl-num">' + dels[pi].num + '</span><span class="dl-content">' + dHl + '</span></div>';
          totalLines++;
        }
        for (var ai = 0; ai < adds.length; ai++) {
          var aHl;
          if (ai < pairs) {
            var wd2 = wordDiffLine(dels[ai].text, adds[ai].text);
            aHl = wd2.newHtml;
          } else {
            aHl = canHL ? highlightLine(adds[ai].text, ext) : escapeHtml(adds[ai].text);
          }
          viewerHtml += '<div class="diff-line added"><span class="dl-num">' + adds[ai].num + '</span><span class="dl-content">' + aHl + '</span></div>';
          totalLines++;
        }
        continue;
      }

      // Standalone add (no preceding del).
      if (e.type === 'add') {
        var sHl = canHL ? highlightLine(e.text, ext) : escapeHtml(e.text);
        viewerHtml += '<div class="diff-line added"><span class="dl-num">' + e.num + '</span><span class="dl-content">' + sHl + '</span></div>';
        totalLines++;
      }
    }
    viewerHtml += '</div>';

    var collapsed = totalLines > 8;
    var wrapCls = 'build-embed' + (collapsed ? ' collapsed' : '');
    var toggleBtn = '<span class="build-embed-chevron">&#x25B6;</span>';

    var diffWrapBtn = '<button class="build-embed-wrap-toggle" title="Toggle line wrapping"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4h10M3 8h7a2 2 0 010 4H8l1.5-1.5M3 12h3"/></svg>wrap</button>';
    return '<div class="' + wrapCls + '" data-embed-type="diff">' +
      '<div class="build-embed-header">' +
        toggleBtn +
        '<span class="build-embed-path">' + escapeHtml(path) + '</span>' +
        diffWrapBtn +
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
      var fenceMatch = line.match(/^```([\w.*-]*)\s*$/);
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
        var wrapBtn = '<button class="md-code-wrap-toggle" title="Toggle line wrapping"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4h10M3 8h7a2 2 0 010 4H8l1.5-1.5M3 12h3"/></svg>wrap</button>';
        var copyBtn = '<button class="md-code-copy" title="Copy code"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="5" width="9" height="9" rx="1.5"/><path d="M5 11H3.5A1.5 1.5 0 012 9.5v-7A1.5 1.5 0 013.5 1h7A1.5 1.5 0 0112 2.5V5"/></svg>copy</button>';
        var header = '<div class="md-code-header">' + langLabel + wrapBtn + copyBtn + '</div>';
        out.push(
          '<div class="md-code-block"' + langAttr + '>' +
          header +
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
      if (paraLines.length === 0) {
        // Safety: skip unrecognised lines so we never infinite-loop.
        out.push('<p>' + inlineMarkdown(escapeHtml(line)) + '</p>');
        i++;
      } else {
        out.push('<p>' + inlineMarkdown(escapeHtml(paraLines.join('\n'))) + '</p>');
      }
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
export { renderMarkdown, wordDiffLine };
