# Terminal

A native AppCore/ViewKit terminal workspace for mochiOS. Tabs
own independent PTYs; shell output is interpreted by a bounded VT screen rather
than stripped into a text log. The interface uses the system's existing omochi
controls, symbols, typography and surfaces.

Open Terminal from `mmake ui` for the Linux desktop preview. `mmake ui-check`
builds the preview applications without starting the desktop. `mmake terminal`
builds the native application.

## Everyday use

- **+** beside the tabs starts the default profile. **▼** opens profiles and
  terminal actions. Each tab has a **×** button that closes only that session.
  Closing an inactive tab preserves the active session. Long titles use a measured
  ellipsis and retain their full accessible name.
- **Command palette** in the ▼ menu searches available actions and profiles. Use Up/Down to choose,
  Enter to run, and Escape to dismiss.
- **Find** in the ▼ menu searches literal, case-sensitive text across the bounded scrollback.
  Enter finds the next result; Shift+Enter finds the previous result, with wrapping.
- Drag to select text. Copy preserves the selected text even when output changes.
  Hold Shift to select or scroll locally when a TUI has enabled mouse reporting.
- Multiline pastes require confirmation because a shell may execute their lines.
  Paste input is limited to 60,000 bytes, never silently truncated, and strips
  control characters other than tabs and line breaks. Bracketed paste is used
  when the application requests it.
- Closing running sessions requires confirmation by default. Completed sessions
  remain readable and report their exit code or signal. Output is drained before
  the completion message appears.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| Ctrl+Shift+T | New default-profile tab |
| Ctrl+Shift+W | Close active tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+Shift+PageUp / PageDown | Move tab left / right |
| Ctrl+Shift+P | Commands |
| Ctrl+Shift+F | Find |
| Ctrl+Shift+C / V | Copy / paste |
| Ctrl+Shift+A | Select visible output |
| Ctrl+Shift+K | Clear terminal screen and scrollback |
| Shift+PageUp / PageDown | Scroll one screen |
| Shift+Home / End | Oldest retained output / live output |
| Ctrl++ / Ctrl+- / Ctrl+0 | Increase / decrease / reset text size |
| Ctrl+, | Settings |

Plain Ctrl+C, Ctrl+D, Ctrl+Z, Tab, arrow keys, F1–F12 and Insert remain terminal
input. The developer shortcuts above are reserved by the workspace.

## Settings and profiles

Settings contains text size (10–32 px), System/Light/Dark terminal colors, cursor
shape, scrollback (100–10,000 lines), copy-on-selection and close confirmation.
Save applies appearance changes; Cancel discards edits. Profile and history-limit
changes apply to new sessions. Text zoom changes the current workspace until a
settings save. Terminal color choices do not change application chrome or OS
colors.

A profile contains a name, absolute executable path, literal arguments and an
optional starting directory. Quoting groups arguments; Terminal performs no
variable expansion, command substitution or shell evaluation. Native executables
still require the existing verified application launcher to authorize them.

Settings are private UTF-8 TOML, validated on load and written with create-new,
0600 permissions and atomic replacement. Invalid settings produce a visible
notice and defaults without overwriting the file. Paths:

- mochiOS: `/var/config/applications/org.mochios.terminal/users/<USER>/settings.toml`
- Linux desktop preview: `$MOCHIOS_PREVIEW_CONFIG_ROOT/terminal/settings.toml`
- Standalone Linux: `$XDG_CONFIG_HOME/mochios/terminal/settings.toml`, falling back
  to `$HOME/.config/mochios/terminal/settings.toml`

Limits are 12 profiles, 12 tabs per workspace.

## Terminal and platform behavior

The VT parser supports cursor positioning, erase/insert operations, ANSI 16/256
and truecolor, Unicode/wide cells, alternate screen buffers, application cursor
keys and bracketed paste. Mouse protocols include X10/VT200, button/any motion,
and SGR/UTF-8 encodings. Escape sequences cannot launch commands or access the
clipboard (OSC 52 remains disabled). Titles are sanitized and bounded. Input uses
a bounded partial-write queue; output reads are bounded per poll.

The mochiOS transport retains the authenticated POSIX launch scope and verified
Capability broker path. Only the PTY slave descriptors are delegated to the child.
The Linux transport uses a real controlling PTY, exact argv and a separate process
session. Neither path fakes a shell. Closing a tab releases its PTY and reaps the
child.

Current boundaries compared with a full Windows Terminal implementation:

- Native starting directories inherit the authenticated POSIX launch scope;
  per-profile starting directories are currently available on Linux only. Use
  `cd` inside the native shell. No process-global chdir workaround is used.
- Resize updates the VT grid and PTY but does not reflow existing wrapped history.
- The text backend does not yet provide italic terminal faces or terminal graphics
  protocols such as Sixel/Kitty. This is not a claim of complete xterm conformance.
- Linux clipboard operations use the existing isolated preview-session clipboard,
  shared with native preview controls, rather than the host desktop clipboard.
- Saved workspace/session restoration, custom keybinding editing and a GPU
  terminal-specific glyph cache are not implemented.

## Validation

Tests cover VT operations, UTF-8/wide cells, alternate screens, search navigation,
escape-sequence isolation, literal arguments, atomic settings writes, partial
input writes, independent tab ownership, native UI keyboard/modal behavior,
paste confirmation and rendering at small/large sizes in both appearances.
Linux tests execute real PTY children to verify working directories, ANSI output,
resize, Ctrl+C, complete output draining, exit status and child cleanup.
