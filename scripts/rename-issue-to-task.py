#!/usr/bin/env python3
"""Rename Build's "issue" to "task" across the repository (#190).

Build's own entity was called an issue, which collides with GitHub's. This
script is the rename, so that rebasing onto a branch written before it means
running it again rather than resolving thousands of conflicts:

    scripts/rename-issue-to-task.py            # rewrite the tree in place
    scripts/rename-issue-to-task.py --check    # exit 1 if a run would change anything
    scripts/rename-issue-to-task.py --kept     # every "issue" left, grouped by why

It rewrites every tracked text file, `git mv`s every tracked path with the
word in it, and runs `cargo fmt` over the bridge. Words are matched by case
and shape, never blindly: `issue`, `issues`, `Issue`, `ISSUE` inside
identifiers (`issue_id`, `trackerIssue`, `TRACKER_ISSUES`) become `task`,
`tasks`, `Task`, `TASK`, and "an issue" becomes "a task", across a line break
and a comment marker too. `issued`, `issuer`, `tissue` and `reissue` are other
words and never match. The comment and event id prefixes `ic-` and `ie-`
become `tc-` and `te-`.

What stays is listed in `rename-issue-to-task.keep` beside this script: GitHub's
issues, third-party names, English that is not about the entity, and the
compatibility code that has to name the old spellings. Each rule protects the
spans its regex matches in the files its glob names; `--kept` prints every
remaining hit under the rule that kept it, which is the allow-list.

A second run is a no-op: once the tree is renamed the only hits left are kept.

The branch is four layers, and rebasing it onto a new base is rebuilding them
there rather than resolving the rename's thousands of lines by hand:

    git checkout -B build/issue-to-task <new base>
    git checkout <old tip> -- scripts/rename-issue-to-task.py scripts/rename-issue-to-task.keep
    git commit                          # 1. this script and its keep list
    git cherry-pick <prep>              # 2. the hand edits that free the word "task"
    scripts/rename-issue-to-task.py     # 3. the rename itself: commit it whole
    git cherry-pick <rename>..<old tip> # 4. the hand edits after it (compat, migration)

A keep rule names only what those hand edits wrote, never the base's own
spellings, so step 3 on any base leaves step 4 applying cleanly.
"""

from __future__ import annotations

import argparse
import fnmatch
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve()
KEEP_FILE = HERE.with_suffix(".keep")

BINARY_SUFFIXES = {
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".icns", ".mp4", ".webm",
    ".woff", ".woff2", ".ttf", ".otf", ".pdf", ".zip", ".gz", ".blend", ".glb",
    ".wasm", ".avif", ".mov",
}

# The word in every shape an identifier or a sentence gives it. Each form stops
# before a following letter of its own case, so `issued`, `issuer`, `ISSUER`
# and `issueview` are left to the compounds below or alone; `\n`, `\t`, `\r`
# and `\0` escapes in a string literal count as a boundary.
ESCAPE = r"(?:(?<=\\n)|(?<=\\t)|(?<=\\r)|(?<=\\0))"
WORD = re.compile(
    r"(?P<article>(?<![A-Za-z0-9])(?:an|An)(?P<gap>[ \t]+|_|[ \t]*\n[ \t]*(?:///?!?|#|\*|--|>)?[ \t]*))?"
    r"(?P<word>"
    rf"(?:(?<![A-Za-z])|{ESCAPE})issue(?=s?(?![a-z]))"
    r"|(?<![A-Z])Issue(?=s?(?![a-z]))"
    r"|(?<![A-Z])ISSUE(?=S?(?![A-Z]))"
    # Lower-case compounds: element ids and a class written as one word.
    r"|(?<=watchagent)issue(?=s)"
    r"|(?<=approve)issue(?![a-z])"
    r"|issue(?=view|delete)"
    # A branch name inside a URL-encoded path: `build%2Fissues-spa`.
    r"|(?<=%2F)issue(?=s?(?![a-z]))"
    r")"
)
REPLACEMENT = {"issue": "task", "Issue": "Task", "ISSUE": "TASK"}

# `ic-` (issue comment) and `ie-` (issue event) ids, the prefixes alone, a
# comment's `#comment-ic-…` anchor, and the prefixes as quoted strings a test
# builds ids from.
ID_PREFIX = re.compile(
    r"(?:(?<![A-Za-z0-9_-])|(?<=#comment-))(?P<prefix>ic|ie)-(?=[0-9A-Za-z…$`\"'{<]|$)"
    r"|(?<=[\"'`])(?P<bare>ic|ie)(?=[\"'`])",
    re.M,
)
ID_REPLACEMENT = {"ic": "tc", "ie": "te"}

# What `--kept` and `--check` count as a remaining hit: any form of the word
# the rewrite would have touched, plus the id prefixes.
REMAINING = re.compile(rf"{WORD.pattern}|{ID_PREFIX.pattern}", re.M)


# Sorted lists the rename puts out of order (`issues.*` sorted before
# `models.*`, `tasks.*` after `settings.*`): the file and the key of each.
SORTED_JSON_LISTS = {"fixtures/api/v1/session.hello.json": "capabilities"}


def resort(text: str, key: str) -> str:
    """Sort the one-string-per-line JSON array under `key`, as written."""
    array = re.compile(rf'(?P<head>^(?P<indent>[ \t]*)"{key}": \[\n)(?P<body>.*?)(?P<tail>\n(?P=indent)\])', re.M | re.S)

    def ordered(match: re.Match) -> str:
        lines = match.group("body").split("\n")
        pad = re.match(r"[ \t]*", lines[0]).group(0)
        values = sorted(line.strip().rstrip(",") for line in lines)
        return match.group("head") + ",\n".join(pad + value for value in values) + match.group("tail")

    return array.sub(ordered, text)


@dataclass
class Rule:
    group: str
    glob: str
    pattern: re.Pattern | None  # None keeps the whole file, path included
    source: str
    hits: int = 0


@dataclass
class KeepList:
    reasons: dict[str, str] = field(default_factory=dict)
    rules: list[Rule] = field(default_factory=list)

    def for_path(self, path: str) -> list[Rule]:
        """A glob names a file by its name before the rename or after it, so a
        rule still holds once the file it protects has been moved."""
        names = {path, rename_path(path)}
        return [rule for rule in self.rules if any(fnmatch.fnmatchcase(name, rule.glob) for name in names)]

    def keeps_whole(self, path: str) -> Rule | None:
        return next((rule for rule in self.for_path(path) if rule.pattern is None), None)


def read_keep_list(path: Path) -> KeepList:
    """`@group<TAB>reason` names a group; `group<TAB>glob<TAB>regex` is a rule,
    and a rule with no regex keeps the whole file. `#` starts a comment line."""
    keep = KeepList()
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.rstrip("\n")
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        fields = line.split("\t")
        if fields[0].startswith("@"):
            keep.reasons[fields[0][1:]] = fields[1]
            continue
        group, glob = fields[0], fields[1]
        if group not in keep.reasons:
            sys.exit(f"{path.name}:{number}: group {group!r} has no @{group} reason")
        regex = fields[2] if len(fields) > 2 and fields[2] else None
        pattern = re.compile(regex, re.M) if regex else None
        keep.rules.append(Rule(group, glob, pattern, f"{path.name}:{number}"))
    return keep


def tracked_files() -> list[str]:
    out = subprocess.run(["git", "ls-files", "-z"], capture_output=True, check=True).stdout
    return [name for name in out.decode().split("\0") if name]


def is_text(path: str, data: bytes) -> bool:
    return Path(path).suffix.lower() not in BINARY_SUFFIXES and b"\0" not in data[:8192]


def protected_spans(text: str, rules: list[Rule]) -> list[tuple[int, int, Rule]]:
    spans = []
    for rule in rules:
        for match in rule.pattern.finditer(text):
            spans.append((match.start(), match.end(), rule))
    return spans


def keeper(position: int, spans: list[tuple[int, int, Rule]]) -> Rule | None:
    return next((rule for start, end, rule in spans if start <= position < end), None)


def rename_word(match: re.Match) -> str:
    word = match.group("word")
    article = match.group("article")
    renamed = REPLACEMENT[word]
    if article:
        return article[0] + match.group("gap") + renamed
    return renamed


def hit_start(match: re.Match) -> int:
    return match.start("word") if match.group("word") else match.start()


def rename_hit(match: re.Match) -> str:
    if match.group("word"):
        return rename_word(match)
    if match.group("bare"):
        return ID_REPLACEMENT[match.group("bare")]
    return ID_REPLACEMENT[match.group("prefix")] + "-"


def rewrite(text: str, spans: list[tuple[int, int, Rule]]) -> str:
    return REMAINING.sub(lambda match: match.group(0) if keeper(hit_start(match), spans) else rename_hit(match), text)


def rename_path(path: str) -> str:
    return "/".join(WORD.sub(rename_word, part) for part in path.split("/"))


def rewrite_contents(files: list[str], keep: KeepList, write: bool) -> list[str]:
    changed = []
    for path in files:
        if keep.keeps_whole(path):
            continue
        data = Path(path).read_bytes()
        if not is_text(path, data):
            continue
        text = data.decode("utf-8")
        rules = [rule for rule in keep.for_path(path) if rule.pattern is not None]
        renamed = rewrite(text, protected_spans(text, rules))
        if path in SORTED_JSON_LISTS:
            renamed = resort(renamed, SORTED_JSON_LISTS[path])
        if renamed != text:
            changed.append(path)
            if write:
                Path(path).write_text(renamed, encoding="utf-8")
    return changed


def rename_paths(files: list[str], keep: KeepList, write: bool) -> list[tuple[str, str]]:
    moves = []
    taken = set(files)
    for path in files:
        if keep.keeps_whole(path):
            continue
        target = rename_path(path)
        if target == path:
            continue
        if target in taken:
            sys.exit(f"cannot rename {path}: {target} already exists")
        moves.append((path, target))
    if write:
        for source, target in moves:
            Path(target).parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(["git", "mv", source, target], check=True)
    return moves


def remaining_hits(files: list[str], keep: KeepList):
    """Every hit left in the tree and the rule keeping it (None: nothing did)."""
    for path in files:
        whole = keep.keeps_whole(path)
        data = Path(path).read_bytes()
        text = data.decode("utf-8") if is_text(path, data) else ""
        if whole:
            for match in REMAINING.finditer(text):
                whole.hits += 1
                yield path, text, match, whole
            if REMAINING.search(path):
                yield path, None, None, whole
            continue
        rules = [rule for rule in keep.for_path(path) if rule.pattern is not None]
        spans = protected_spans(text, rules)
        for match in REMAINING.finditer(text):
            rule = keeper(hit_start(match), spans)
            if rule:
                rule.hits += 1
            yield path, text, match, rule


def line_of(text: str, position: int) -> tuple[int, str]:
    start = text.rfind("\n", 0, position) + 1
    end = text.find("\n", position)
    return text.count("\n", 0, position) + 1, text[start:end if end >= 0 else len(text)]


def report_kept(files: list[str], keep: KeepList) -> int:
    by_group: dict[str, list[str]] = {}
    loose = []
    for path, text, match, rule in remaining_hits(files, keep):
        if text is None:
            entry = f"{path} (path)"
        else:
            number, line = line_of(text, match.start())
            entry = f"{path}:{number}: {line.strip()[:160]}"
        if rule is None:
            loose.append(entry)
        else:
            by_group.setdefault(rule.group, []).append(entry)
    for group, entries in by_group.items():
        print(f"## {group} ({len(entries)}): {keep.reasons[group]}")
        for entry in dict.fromkeys(entries):
            print(f"  {entry}")
    unused = [rule for rule in keep.rules if rule.hits == 0]
    for rule in unused:
        print(f"unused rule {rule.source}: {rule.group} {rule.glob}", file=sys.stderr)
    for entry in loose:
        print(f"not renamed and not kept: {entry}", file=sys.stderr)
    return 1 if loose else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="change nothing; exit 1 if a run would")
    mode.add_argument("--kept", action="store_true", help="list every remaining hit by the rule keeping it")
    args = parser.parse_args()

    root = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True)
    os.chdir(root.stdout.strip())
    keep = read_keep_list(KEEP_FILE)
    files = tracked_files()
    if args.kept:
        return report_kept(files, keep)
    changed = rewrite_contents(files, keep, write=not args.check)
    moves = rename_paths(files, keep, write=not args.check)
    if not args.check and any(path.startswith("bridge/") for path in changed):
        # The new word is a letter shorter, so rustfmt joins lines it had split:
        # formatting is part of the rename's output, not a follow-up to it.
        subprocess.run(["cargo", "fmt"], cwd="bridge", check=True)
    verb = "would change" if args.check else "changed"
    print(f"{verb} {len(changed)} files, {'would move' if args.check else 'moved'} {len(moves)} paths")
    return 1 if args.check and (changed or moves) else 0


if __name__ == "__main__":
    sys.exit(main())
