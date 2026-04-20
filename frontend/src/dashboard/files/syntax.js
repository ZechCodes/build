import { escHtml } from '../util/html.js';

export const SYNTAX_RULES = {
  js: [
    [/\b(const|let|var|function|return|if|else|for|while|class|import|export|from|default|async|await|new|this|throw|try|catch|finally|switch|case|break|continue|typeof|instanceof|in|of|yield|void|delete)\b/g, 'keyword'],
    [/(["'`])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?|0x[\da-f]+|0b[01]+|0o[0-7]+)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  py: [
    [/\b(def|class|return|if|elif|else|for|while|import|from|as|with|try|except|finally|raise|yield|lambda|pass|break|continue|and|or|not|is|in|True|False|None|async|await|self)\b/g, 'keyword'],
    [/("""[\s\S]*?"""|'''[\s\S]*?'''|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?|0x[\da-f]+|0b[01]+|0o[0-7]+)\b/gi, 'number'],
    [/@\w+/g, 'func'],
  ],
  html: [
    [/<!--[\s\S]*?-->/g, 'comment'],
    [/(<\/?)([\w-]+)/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\b(\w+)=/g, 'attr'],
  ],
  css: [
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\b(\d+\.?\d*)(px|em|rem|%|vh|vw|s|ms)?\b/g, 'number'],
    [/([.#][\w-]+)/g, 'func'],
    [/\b(color|background|display|flex|grid|margin|padding|border|font|width|height|position|top|left|right|bottom|z-index|overflow|opacity|transition|transform)\b/g, 'keyword'],
  ],
  rs: [
    [/\b(fn|let|mut|const|pub|struct|enum|impl|trait|use|mod|crate|self|super|match|if|else|for|while|loop|return|break|continue|where|async|await|move|type|as|in|ref|unsafe|extern|dyn|static)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?|0x[\da-f]+|0b[01]+|0o[0-7]+)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  go: [
    [/\b(func|var|const|type|struct|interface|map|chan|go|select|case|default|if|else|for|range|return|break|continue|switch|package|import|defer|nil|true|false|make|new|len|cap|append|copy|close|delete|panic|recover)\b/g, 'keyword'],
    [/(["'`])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?|0x[\da-f]+|0b[01]+|0o[0-7]+)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  yaml: [
    [/#.*$/gm, 'comment'],
    [/^(\s*)([\w][\w.\-\/]*)(\s*:)/gm, 'attr'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\b(true|false|yes|no|null|~)\b/gi, 'keyword'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/(\$\w+|\$\{[^}]+\})/g, 'func'],
    [/^(\s*-)\s/gm, 'operator'],
  ],
  toml: [
    [/#.*$/gm, 'comment'],
    [/^\s*\[+[\w.\-"]+\]+/gm, 'type'],
    [/^(\s*)([\w][\w.\-]*)(\s*=)/gm, 'attr'],
    [/("""[\s\S]*?"""|'''[\s\S]*?'''|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g, 'string'],
    [/\b(true|false)\b/g, 'keyword'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}:\d{2})?/g, 'number'],
  ],
  sh: [
    [/#.*$/gm, 'comment'],
    [/\b(if|then|else|elif|fi|for|while|do|done|case|esac|in|function|return|exit|local|export|source|set|unset|readonly|declare|typeset|shift|eval|exec|trap)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/(\$\w+|\$\{[^}]+\}|\$\([^)]+\))/g, 'func'],
    [/\b(\d+)\b/g, 'number'],
    [/[|&;><]{1,2}/g, 'operator'],
  ],
  docker: [
    [/#.*$/gm, 'comment'],
    [/^(FROM|RUN|CMD|LABEL|EXPOSE|ENV|ADD|COPY|ENTRYPOINT|VOLUME|USER|WORKDIR|ARG|ONBUILD|STOPSIGNAL|HEALTHCHECK|SHELL|MAINTAINER|AS)\b/gmi, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/(\$\w+|\$\{[^}]+\})/g, 'func'],
    [/\b(\d+)\b/g, 'number'],
  ],
  c: [
    [/\b(auto|break|case|char|const|continue|default|do|double|else|enum|extern|float|for|goto|if|inline|int|long|register|restrict|return|short|signed|sizeof|static|struct|switch|typedef|union|unsigned|void|volatile|while|_Bool|_Complex|_Imaginary|bool|true|false|NULL|nullptr)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/#\s*(include|define|ifdef|ifndef|endif|if|else|elif|undef|pragma|error|warning)\b/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFlLuU]*|0x[\da-f]+[lLuU]*|0b[01]+[lLuU]*)\b/gi, 'number'],
    [/\b([A-Z][\w]*_[\w]*|[A-Z]{2,})\b/g, 'type'],
  ],
  cpp: [
    [/\b(alignas|alignof|auto|bool|break|case|catch|char|char8_t|char16_t|char32_t|class|concept|const|consteval|constexpr|constinit|continue|co_await|co_return|co_yield|decltype|default|delete|do|double|dynamic_cast|else|enum|explicit|export|extern|false|float|for|friend|goto|if|inline|int|long|mutable|namespace|new|noexcept|nullptr|operator|override|private|protected|public|register|requires|return|short|signed|sizeof|static|static_assert|static_cast|struct|switch|template|this|thread_local|throw|true|try|typedef|typeid|typename|union|unsigned|using|virtual|void|volatile|wchar_t|while)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/#\s*(include|define|ifdef|ifndef|endif|if|else|elif|undef|pragma|error|warning)\b/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFlLuU]*|0x[\da-f]+[lLuU]*|0b[01]+[lLuU]*)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  cs: [
    [/\b(abstract|as|base|bool|break|byte|case|catch|char|checked|class|const|continue|decimal|default|delegate|do|double|else|enum|event|explicit|extern|false|finally|fixed|float|for|foreach|goto|if|implicit|in|int|interface|internal|is|lock|long|namespace|new|null|object|operator|out|override|params|private|protected|public|readonly|ref|return|sbyte|sealed|short|sizeof|stackalloc|static|string|struct|switch|this|throw|true|try|typeof|uint|ulong|unchecked|unsafe|ushort|using|var|virtual|void|volatile|while|async|await|yield|record|init|required|global)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\$"[^"]*"/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/#\s*(if|else|elif|endif|region|endregion|define|undef|pragma|nullable)\b/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFdDmM]?|0x[\da-f]+[lLuU]*)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
    [/\[\w+\]/g, 'attr'],
  ],
  bat: [
    [/\bREM\b.*$/gmi, 'comment'],
    [/^\s*::\s.*$/gm, 'comment'],
    [/\b(echo|set|if|else|goto|call|exit|for|in|do|not|exist|defined|errorlevel|pause|cls|rem|setlocal|endlocal|enabledelayedexpansion|pushd|popd|shift|start|choice|timeout)\b/gi, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/(%\w+%|%~?\d|!\w+!)/g, 'func'],
    [/\b(\d+)\b/g, 'number'],
    [/^\s*:\w+/gm, 'type'],
  ],
  java: [
    [/\b(abstract|assert|boolean|break|byte|case|catch|char|class|const|continue|default|do|double|else|enum|extends|final|finally|float|for|goto|if|implements|import|instanceof|int|interface|long|native|new|null|package|private|protected|public|return|short|static|strictfp|super|switch|synchronized|this|throw|throws|transient|try|void|volatile|while|true|false|var|record|sealed|permits|yield)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/@\w+/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFdDlL]?|0x[\da-f]+[lL]?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  rb: [
    [/\b(def|class|module|if|elsif|else|unless|case|when|while|until|for|do|end|begin|rescue|ensure|raise|return|yield|block_given\?|require|require_relative|include|extend|attr_accessor|attr_reader|attr_writer|self|super|nil|true|false|and|or|not|in|then|puts|print|lambda|proc)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/(:\w+)/g, 'attr'],
    [/(@\w+)/g, 'func'],
  ],
  php: [
    [/\b(abstract|and|array|as|break|callable|case|catch|class|clone|const|continue|declare|default|do|echo|else|elseif|empty|enddeclare|endfor|endforeach|endif|endswitch|endwhile|enum|eval|exit|extends|final|finally|fn|for|foreach|function|global|goto|if|implements|include|include_once|instanceof|insteadof|interface|isset|list|match|namespace|new|null|or|print|private|protected|public|readonly|require|require_once|return|static|switch|this|throw|trait|try|unset|use|var|while|xor|yield|true|false|self)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/#.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\$\w+/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  swift: [
    [/\b(actor|associatedtype|async|await|break|case|catch|class|continue|default|defer|deinit|do|else|enum|extension|fallthrough|fileprivate|for|func|guard|if|import|in|init|inout|internal|is|let|nil|open|operator|private|protocol|public|repeat|rethrows|return|self|Self|static|struct|subscript|super|switch|throw|throws|try|typealias|var|weak|where|while|true|false|some|any)\b/g, 'keyword'],
    [/("""|"(?:[^"\\]|\\.)*")/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/@\w+/g, 'attr'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  dart: [
    [/\b(abstract|as|assert|async|await|base|break|case|catch|class|const|continue|covariant|default|deferred|do|dynamic|else|enum|export|extends|extension|external|factory|false|final|finally|for|Function|get|hide|if|implements|import|in|interface|is|late|library|mixin|new|null|on|operator|part|required|rethrow|return|sealed|set|show|static|super|switch|sync|this|throw|true|try|typedef|var|void|when|while|with|yield)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/@\w+/g, 'attr'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  scala: [
    [/\b(abstract|case|catch|class|def|do|else|enum|export|extends|extension|false|final|finally|for|forSome|given|if|implicit|import|lazy|match|new|null|object|override|package|private|protected|return|sealed|super|this|then|throw|trait|true|try|type|using|val|var|while|with|yield)\b/g, 'keyword'],
    [/("""|"(?:[^"\\]|\\.)*")/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/@\w+/g, 'attr'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[fFdDlL]?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  lua: [
    [/\b(and|break|do|else|elseif|end|false|for|function|goto|if|in|local|nil|not|or|repeat|return|then|true|until|while)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/--\[\[[\s\S]*?\]\]/g, 'comment'],
    [/--.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
  ],
  sql: [
    [/\b(SELECT|FROM|WHERE|INSERT|INTO|UPDATE|SET|DELETE|CREATE|ALTER|DROP|TABLE|INDEX|VIEW|JOIN|INNER|LEFT|RIGHT|OUTER|FULL|CROSS|ON|AND|OR|NOT|IN|IS|NULL|AS|ORDER|BY|GROUP|HAVING|LIMIT|OFFSET|UNION|ALL|DISTINCT|EXISTS|BETWEEN|LIKE|CASE|WHEN|THEN|ELSE|END|BEGIN|COMMIT|ROLLBACK|TRANSACTION|PRIMARY|KEY|FOREIGN|REFERENCES|DEFAULT|CHECK|UNIQUE|CONSTRAINT|VALUES|COUNT|SUM|AVG|MIN|MAX|CASCADE|IF|FUNCTION|PROCEDURE|TRIGGER|GRANT|REVOKE|WITH|RECURSIVE|OVER|PARTITION|RANK|ROW_NUMBER|COALESCE|CAST|CONVERT)\b/gi, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/--.*$/gm, 'comment'],
    [/\/\*[\s\S]*?\*\//g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
  ],
  r: [
    [/\b(if|else|repeat|while|function|for|in|next|break|TRUE|FALSE|NULL|Inf|NaN|NA|NA_integer_|NA_real_|NA_complex_|NA_character_|return|invisible|library|require|source|stop|warning|message|cat|print|paste|paste0|sprintf)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?[iL]?)\b/gi, 'number'],
    [/<-|->|<<-|->>|%%|%in%|%\*%/g, 'operator'],
  ],
  perl: [
    [/\b(my|our|local|sub|if|elsif|else|unless|while|until|for|foreach|do|last|next|redo|return|use|require|package|BEGIN|END|die|warn|print|say|chomp|chop|push|pop|shift|unshift|sort|reverse|map|grep|join|split|open|close|read|write|defined|undef|exists|delete|ref|bless|tie|untie|eval|qw)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/(\$[\w:]+|@[\w:]+|%[\w:]+)/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/=~|!~|&&|\|\||\/\//g, 'operator'],
  ],
  elixir: [
    [/\b(def|defp|defmodule|defmacro|defmacrop|defstruct|defprotocol|defimpl|defguard|defdelegate|do|end|if|else|unless|case|cond|when|with|for|in|fn|raise|rescue|catch|after|try|receive|send|spawn|import|use|alias|require|true|false|nil|and|or|not|is_atom|is_binary|is_boolean|is_float|is_function|is_integer|is_list|is_map|is_nil|is_number|is_pid|is_tuple)\b/g, 'keyword'],
    [/("""|"(?:[^"\\]|\\.)*")/g, 'string'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/#.*$/gm, 'comment'],
    [/(:\w+)/g, 'attr'],
    [/@\w+/g, 'func'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
    [/\|>/g, 'operator'],
  ],
  erlang: [
    [/\b(after|and|andalso|band|begin|bnot|bor|bsl|bsr|bxor|case|catch|div|end|fun|if|let|not|of|or|orelse|receive|rem|try|when|xor|true|false|undefined)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/%.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
    [/\b(\w+):/g, 'func'],
  ],
  zig: [
    [/\b(align|allowzero|and|anyframe|anytype|asm|async|await|break|callconv|catch|comptime|const|continue|defer|else|enum|errdefer|error|export|extern|fn|for|if|inline|linksection|noalias|nosuspend|null|opaque|or|orelse|packed|pub|resume|return|struct|suspend|switch|test|threadlocal|true|false|try|undefined|union|unreachable|var|volatile|while)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\/\/.*$/gm, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
  ],
  haskell: [
    [/\b(as|case|class|data|default|deriving|do|else|family|forall|foreign|hiding|if|import|in|infix|infixl|infixr|instance|let|module|newtype|of|qualified|then|type|where|True|False|Nothing|Just|Left|Right|IO|Maybe|Either|String|Int|Integer|Float|Double|Bool|Char)\b/g, 'keyword'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/--.*$/gm, 'comment'],
    [/\{-[\s\S]*?-\}/g, 'comment'],
    [/\b(\d+\.?\d*(?:e[+-]?\d+)?)\b/gi, 'number'],
    [/\b([A-Z]\w*)\b/g, 'type'],
    [/::|=>|->|<-|\.\./g, 'operator'],
  ],
  make: [
    [/#.*$/gm, 'comment'],
    [/^[\w.\-\/]+\s*[:+?]?=/gm, 'attr'],
    [/^[\w.\-\/\%]+\s*:/gm, 'type'],
    [/\$[\(\{][\w@<^+*?%]+[\)\}]/g, 'func'],
    [/\$[@<^+*?%]/g, 'func'],
    [/(["'])(?:(?!\1|\\).|\\.)*?\1/g, 'string'],
    [/\b(ifeq|ifneq|ifdef|ifndef|else|endif|include|override|export|unexport|define|endef|vpath)\b/g, 'keyword'],
  ],
};

// Map file extensions to language.
export const EXT_MAP = {
  js: 'js', jsx: 'js', ts: 'js', tsx: 'js', mjs: 'js', cjs: 'js',
  py: 'py', pyi: 'py',
  html: 'html', htm: 'html', xml: 'html', svg: 'html',
  css: 'css', scss: 'css', less: 'css',
  rs: 'rs',
  go: 'go',
  json: 'js', jsonc: 'js',
  sh: 'sh', bash: 'sh', zsh: 'sh',
  yml: 'yaml', yaml: 'yaml',
  toml: 'toml', ini: 'toml',
  c: 'c', h: 'c',
  cpp: 'cpp', cxx: 'cpp', cc: 'cpp', hpp: 'cpp', hxx: 'cpp', hh: 'cpp',
  cs: 'cs', csx: 'cs',
  java: 'java', kt: 'java', kts: 'java',
  rb: 'rb', rake: 'rb', gemspec: 'rb',
  bat: 'bat', cmd: 'bat',
  dockerfile: 'docker',
  php: 'php', phtml: 'php',
  swift: 'swift',
  dart: 'dart',
  scala: 'scala', sc: 'scala',
  lua: 'lua',
  sql: 'sql',
  r: 'r',
  pl: 'perl', pm: 'perl', perl: 'perl',
  ex: 'elixir', exs: 'elixir',
  erl: 'erlang', hrl: 'erlang',
  zig: 'zig',
  hs: 'haskell', lhs: 'haskell',
  makefile: 'make', mk: 'make',
};

export function highlightLine(text, ext) {
  const lang = EXT_MAP[ext];
  const rules = lang ? SYNTAX_RULES[lang] : null;
  if (!rules || !text) return escHtml(text || '');

  // Tokenize: find all matches, sort by position, apply non-overlapping.
  const tokens = [];
  for (const [re, cls] of rules) {
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(text)) !== null) {
      tokens.push({ start: m.index, end: m.index + m[0].length, cls, text: m[0] });
    }
  }
  tokens.sort((a, b) => a.start - b.start || b.end - a.end);

  let result = '';
  let pos = 0;
  for (const tok of tokens) {
    if (tok.start < pos) continue; // overlapping, skip
    if (tok.start > pos) result += escHtml(text.slice(pos, tok.start));
    result += `<span class="tok-${tok.cls}">${escHtml(tok.text)}</span>`;
    pos = tok.end;
  }
  if (pos < text.length) result += escHtml(text.slice(pos));
  return result || '&nbsp;';
}
