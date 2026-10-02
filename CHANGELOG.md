# Changelog

Notable changes to the bridge (`build-bridge`). The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the bridge uses
[Semantic Versioning](https://semver.org/). Wire versions are the bridge's RPC
protocol versions; see
[Wire versioning and capabilities](ARCHITECTURE.md#wire-versioning-and-capabilities).

## [0.2.7] - Unreleased

Wire 3.5.0.

### Added

- A moved task action carries its destination column as `task_action.to`
  (wire 3.5.0), so the agent's own conversation line can name it (#323).
- A development build the bridge service runs can be replaced by a release
  from Settings: Install shows a warning naming the release and the restart,
  then sends `bridge.install_update` with `replace_development_build` (wire
  3.5.0, `updates.replaceDevelopmentBuild`). It goes through the same
  verified download and helper, and rollback restores the development build
  and its marker as they were. A bridge started by hand, or a service running
  another executable, is still refused; the panel shows the install script
  command. A development build running from a cargo target directory is
  warned that the next `cargo build` overwrites the release. An install
  queued for when agents are done keeps the confirmation; one queued before
  the binary became a development build never runs (#322).

### Fixed

- Conversation task notices use the new wording on one line, with an ellipsis
  when they run out of room. Actor and assignee names omit the workspace;
  hover text carries the full line and task title (#323).
- `build-bridge pair` retires a stored approval only on the word of the api
  that approved it. The identity file now records that api when a pairing
  through it completes; one paired before that counts as approved by
  `https://getbuild.ing`, whatever another api later says about it, so one
  paired with a self-hosted api needs `pair --retire` after a revoke. Urls
  name the same api when scheme, host (any case) and port (written or
  implied) agree.
  Another api's "not approved" leaves the identity untouched: `pair` names
  the api it asked and exits with status 3, and the installer then offers
  `pair --retire`, which takes that api's word. `pair` refuses any other
  argument (#320).
- Agent sessions and CLI probes the bridge starts no longer inherit
  `BRIDGE_IDENTITY_FILE` or identity keys from its environment, so a tool an
  agent runs under a temporary `HOME` cannot reach this machine's identity.
  A user's own terminal keeps them (#320).
- `install.sh` quotes the commands it prints to paste, and the line that puts
  the install directory on `PATH`, so any install path it accepts works
  pasted into sh, bash or zsh, including one with `$`, a backtick, a quote, a
  backslash or `!`. Run with `BRIDGE_API_URL`, it starts each command it
  prints with that variable, so a pasted `pair` or `install-service` asks
  the same api (#320).
- A device you just approved is usable in seconds rather than 30 s or more.
  The bridge sends its first heartbeat the moment the relay authenticates
  its socket, and again each time the socket comes back, a failed beat's
  retry included, instead of at the next 30 s tick. It dials the relay before
  starting its background services and redials about half a second after a
  socket ends (spread either way, doubling to 30 s). While showing a pairing
  code it asks for its approval every second for two minutes, then every
  2 s; an api answering 429 or 5xx is asked less often, after its
  `Retry-After` when it gives one, and an api out of reach (no connection, or
  a 404 while it is redeployed) is waited for up to two minutes instead of
  ending the pairing. A code the api says is gone (revoked, or no such
  device) ends the pairing after three such answers, saying to run
  `build-bridge pair` again. After an approve, the app reads the device list every
  second until it lists the device online, for up to 90 s (#321).
- A failed update check no longer sticks: the next successful check clears
  it, and it reads "Could not check for updates.", beside the available
  version when one was found. Only a failed install reads "The update
  failed." and lasts until an install succeeds. An error saved by an older
  bridge with no sign an install ran is read as a check's, so the stuck 404
  goes with the next check; a development build drops it at startup (#322).
- A disabled Install button in the update panel no longer looks pressable
  (#322).

### Changed

- Add a device's pairing-link warning reads as a warning, in amber, not as an
  error (#320).
- A development build no longer checks for releases on its own, at startup
  or daily; it checks when asked. Nothing marks it as having an update, and
  it forgets an earlier run's check at startup, so it sends no
  `update_available` an older app would badge (#322).

## [0.2.6] - 2026-10-01

### Changed

- `build-bridge pair` prints a short block that fits 80 columns: the pairing
  code, the fingerprint's first 32 hex digits (128 bits), and one
  `approve at:` link to `/app/#/pair/<code>`. The link opens Add a device
  with this device looked up, after sign-in if needed. The sheet says a link
  opened it and warns that approving gives the machine access to your
  account; approving is still your press. The approve screen shows the same
  32 digits, with the full fingerprint beneath (#319).
- When `pair` sets aside a revoked or unknown pairing, it now says so in two
  lines naming `~/.build`. The README says how to put the old identity back
  (#319).
- `install.sh` drops its "If a pairing code appears" hint. Both installers
  break their lines between words to fit 80 columns, and give commands to
  paste lines of their own (#319).

## [0.2.5] - 2026-10-01

### Changed

- `install.sh` and `install-desktop.sh` print one plain line per step, say
  nothing when cosign is absent (the checksum is still verified), and end
  every error on what to do next; colour only on a terminal without
  `NO_COLOR` (#318).

### Fixed

- The release workflow stages its assets outside the checkout and publishes
  through a draft, so a failed upload leaves no public release (#305).
- Release notes carry this file's entry for the tagged version (#303).
- `build-bridge pair` on a machine whose stored pairing was revoked or is
  unknown to the api no longer dead-ends on "not approved yet": it sets the old
  identity aside, says which api answered and how to undo it, and pairs a new
  device with a fresh code. `install-service` gives the same reason,
  `uninstall-service` says it keeps the identity, and the status check times
  out instead of hanging (#317).

## [0.2.4] - 2026-09-30

Wire 3.4.0.

### Added

- `project.update_source` edits a project source's label, base branch, remote
  and folder (wire 3.2.0, #228).
- Each source's base branch is kept in step with its remote, fast-forward only,
  by a bridge service that never loses or strands work (wire 3.3.0, #267).
- `workspace.measure_sizes` queues a niced size walk per workspace (wire 3.4.0,
  #273).
- A repository with no commits opens as a Git project, and a project agent can
  be spawned and delivered to in a project with no Git (#297).
- Every agent is taught the Build reference shapes (`#42`, `@agent:…`,
  `[[workspace:path]]`) where it writes (#229).

### Changed

- A source's remote is read from its checkout's `origin`, not a stored copy
  (#228).
- A workspace is cut from the fetched commit when its base is checked out
  behind (#271).
- Agents involve the user only when a task needs them, and talk to them on the
  tasks they are on (#232).
- Timer-driven loops start only once the app mutex is held and observed (#243).
- `fs.list` names a missing folder `not_found` (#242).
- Clone URLs point at `ZechCodes/Build`.

### Fixed

- Type changes and untracked files in a replaced directory are judged at a
  cut (#271).
- A size walk landing mid-sweep keeps its newer size (#273).
- The workspace checkouts a remote change could not reach are named (#228).
- A negotiated data channel opens on its first message, not only on the SCTP
  handshake event, so a peer's first message in the same read burst is not
  dropped; the vendored `rtc` carries the patch (#298).
- Closing a workspace settings sheet settles once its last draft is stored, so
  no draft write lands after the sheet is gone (#299).

### Security

- Remotes that git would read as an option or helper are refused, and `--` is
  passed to `clone` and `remote`.
- A remote whose ssh user, host or port starts with a dash, or is
  percent-encoded, is refused (#228).
- Bridge test fixtures read none of the machine's git config, so a global
  `commit.gpgsign` neither signs them with a real key nor prompts (#242).

## [0.2.3] - 2026-09-29

Wire 3.1.0.

### Added

- Issues are renamed to tasks. Store schema 10 migrates a v9 store on first
  open, after a copy of the database; old `issue-` ids resolve at every tool and
  verb (wire 2.0.0, #190).
- The model catalog offers only models the installed CLI can run, refuses
  unrunnable models at spawn, and announces `models.changed` (wire 2.2.0, #203).
  Claude Sonnet 5.5 joins the Claude catalog (#202).
- An agent names the agent whose Build MCP call created it (`agents.createdBy`,
  wire 3.1.0, #216).
- Push content is sealed to each browser's notification key and padded to
  1024 bytes (#200). Pushes go out when an agent dies mid-turn or files
  something that asks the user, and exactly for what adds to the unread
  counter (#191).
- Watched tasks report their `unread_count` in lists and reads (wire 1.29.0,
  #104). A conversation's feed row carries its own session (wire 1.28.0,
  #103).
- Body reads answer a byte range, a page of whole lines at a time (wire 1.26.0,
  #95). `issues.list` pages with a limit and an opaque cursor (wire 1.25.0,
  #85).
- Workspace reclaim: settings hold the idle threshold and the prune switch, and
  reclaim reports what became of each branch (#167, #135).
- A task an agent files can ask the user.

### Changed

- The project agent starts in its project's base and resumes only where a
  session was had (#187).
- Liveness under load: no tokio worker waits on the app mutex, agents' tool
  calls take turns at it, and the flusher and terminal paint run on a push
  runtime (#131). The heartbeat backs off with jitter and honours
  `Retry-After`; the daemon's stderr log rotates past 8 MiB.
- Agent scopes are ordered before the bridge so a stop records who was working
  (#213).
- A child silent past the startup deadline is a refused start.
- Git measurement is bounded, killable, and marked unmeasured on overrun
  (#135).

### Removed

- Dead wire verbs, the legacy `board.changed` and `entity.changed` pushes, the
  dead MCP read actions and the old task scheduler (wire 3.0.0, #207).

### Fixed

- The DTLS ClientHello is sent when ICE selects the pair, and the webrtc
  driver drains what a wake produced before it waits; `webrtc`, `rtc` and
  `rtc-turn` 0.20.4 are vendored with those fixes (#179, #166).
- Linked tasks stay open on an unmerged finish; a branch row prefers the live
  run.
- Done refuses a run in a linked worktree or an adopted checkout (#172).

[0.2.6]: https://github.com/ZechCodes/build-releases/releases/tag/bridge-v0.2.6
[0.2.5]: https://github.com/ZechCodes/build-releases/releases/tag/bridge-v0.2.5
[0.2.4]: https://github.com/ZechCodes/build-releases/releases/tag/bridge-v0.2.4
[0.2.3]: https://github.com/ZechCodes/build-releases/releases/tag/bridge-v0.2.3
