# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

vizin is a terminal binary disassembler/decompiler with vim/nvim-style modal keybindings. It drives
[rizin](https://rizin.re) plus the [rz-ghidra](https://github.com/rizinorg/rz-ghidra) plugin (Ghidra's
real C++ decompiler) as a subprocess, and renders a modal TUI (ratatui) on top: function sidebar,
disassembly listing, Ghidra decompiler pane, strings/imports/exports/segments panels, xrefs popup,
and a hex editor with byte patching.

## Commands

```sh
cargo build --release              # release build (LTO + strip enabled)
cargo build                        # dev build
cargo clippy                       # MUST be clean — see lint policy below
cargo test                         # all tests are unit tests, no integration harness
cargo test <name>                  # run a single test by name/substring, e.g. `cargo test gg_and_gd`

./target/release/vizin /bin/ls                       # read-only
./target/release/vizin -w ./somebinary               # write mode (enables patching)
./target/release/vizin -p session.rzdb /bin/ls       # load/save a rizin project
./target/release/vizin --no-analysis /bin/ls         # skip the initial `aaa` analysis pass
```

Requirements to actually run it: `rizin` (0.8+) on `PATH`, and the `rz-ghidra` plugin for the
decompiler pane (the app degrades gracefully without it — decompiler disabled, everything else works).

### Clippy lint policy (`[lints.clippy]` in Cargo.toml)

This is not the default lint set — `clippy::all`, `pedantic`, and `nursery` are all `deny`, plus a long
explicit list including `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `unimplemented`,
`print_stdout`/`print_stderr`, `missing_const_for_fn`, `cast_possible_truncation`/`cast_sign_loss`, etc.

**When fixing clippy findings, fix the underlying code — don't reach for `#[allow(...)]`.** Convert raw
indexing to `.get()`/`.get_mut()` with real handling; convert `as` casts to `try_from`/`try_into` with a
sane fallback; use slice patterns instead of indexing fixed-length `Layout::split()` results;
`let...else` instead of a match that only returns early. Reach for `#[allow]` only as a genuine last
resort on a single line with a comment explaining why the lint doesn't apply — a handful already exist
in the codebase (e.g. `ts.rs`'s grammar-load `.expect()`, a couple of `module_name_repetitions` allows on
the `*View`/`*Popup` types) and each one has a one-line justification comment. Don't add new broad
file-level `#![allow(...)]` blocks.

## Architecture

### The pipe → backend → app → views layering

```
main.rs   — clap CLI, terminal init/restore (ratatui::init/restore), the event loop
  app.rs  — App: all mutable state, key dispatch, view orchestration (impl App also gains
            its render methods from ui.rs — see below)
    backend.rs — typed Rust API (serde structs) over rizin JSON commands (`Backend::functions()`,
                 `Backend::disasm()`, `Backend::rename_variable()`, etc.)
      pipe.rs  — RzPipe: spawns `rizin -q0 [-w] [-p project] <file>`, sends `cmd\n`, reads until
                 the NUL byte rizin emits as a reply terminator. `cmd()` for raw text, `cmdj()`
                 parses the reply as JSON.
    decompiler.rs — a SECOND, independent RzPipe/rizin subprocess running on its own thread,
                 dedicated to `pdgj` (Ghidra decompile) calls, because decompiling a large function
                 can take seconds and must never block the UI thread. Talks to `app.rs` over
                 mpsc channels (`Req`/`Msg`). Rename/comment edits made in the main pipe are
                 replayed onto this second instance (`forward_edit`) so decompiled output stays
                 in sync with renames.
    vim.rs  — NormalParser: a pure state machine (no I/O, no App access) turning KeyEvents into
              `Action` enum values. Handles counts (`12j`), multi-key sequences (`gg`, `gd`, `zz`,
              `Ctrl-w h`, `f{char}`), all independent of what view is focused. This is the most
              heavily unit-tested module — when adding a keybinding, add it here first.
    ui.rs   — NOT free functions: `impl App { pub fn draw(&mut self, frame) ... }` and its private
              draw_status/draw_cmdline/draw_value_popup helpers. Composes the views/*.rs render
              calls into the overall layout (sidebar | main pane(s) | status bar | cmdline).
    views/  — one struct per pane/popup, each owns its own render() and most of its own input
              handling logic:
      listing.rs   — disassembly (virtualized over `pdj`, extends its buffer as you scroll)
      decomp.rs    — Ghidra pseudo-C pane; colors from `pdgj`'s `syntax_highlight` annotations;
                     maps cursor byte position back to an address via `offset` annotations (this
                     is what makes rename/xrefs/comment work identically in listing AND decompiler)
      functions.rs — sidebar function list with live filter
      panels.rs    — generic list panel reused for strings/imports/exports/segments
      hex.rs       — hex+ASCII grid, nibble-by-nibble patch staging, committed as `wx` runs
      xrefs.rs, completion.rs, help.rs — popups (xrefs-to/from, `:` tab-completion, `?` cheatsheet)
      mod.rs       — shared `Scroller` (cursor/scroll/viewport math used by every list-like view)
                     and `search_lines` (used by 3 different view types — this is why it's a free
                     function rather than a method: no single owner)
    ts.rs   — TsParser: tree-sitter-c parsing of the decompiled pseudo-C, used for AST-aware word
              extraction, bracket matching, and reference-finding that correctly skips strings/
              comments. Caches the last-parsed (source, Tree) pair since re-parsing a
              thousand-line decompiled function on every cursor move would be wasteful.
```

### Conventions this codebase actually follows (worth matching)

- **Free functions get pulled into `impl` blocks when they have a single obvious owner.** Most
  modules follow this already (e.g. `TsParser`'s internal tree-walking helpers, `Backend::from_value`,
  `HelpPopup::help_rows`). A free function is fine when genuinely shared by multiple unrelated owners
  (`search_lines`) or when it's a bootstrap/thread-entry-point with no natural `&self` receiver
  (`main.rs`'s `run`/`splash`, `decompiler.rs`'s `worker`) — don't force a wrapper struct onto those.
- **Ghidra-only decompiler variables (`pcVar8`, `iVar4`, …) have no backing rizin variable.** Renaming
  one for real (`afvn`) is impossible — there's nothing in rizin's analysis to rename. `App` keeps a
  separate `local_aliases` map (persisted to a `<binary>.vizin-aliases.json` sidecar next to the
  binary, NOT through rizin's project format) for display-only renames of these, distinguished via
  `Backend::is_real_variable`.
- **rizin's `afvn` takes `<new_name> <old_name>`** (new first) — the reverse of what you'd guess from
  reading it left-to-right as "rename old to new". This bit us once already; don't re-flip it.
- **The decompiler pane's cursor↔address mapping depends entirely on `pdgj`'s annotation byte
  offsets staying valid.** Never mutate `DecompView`'s underlying code text/line buffer as a way to
  rename/relabel something in place — any length-changing edit desyncs every annotation after that
  point (this is why variable aliases are shown as dim inline "ghost text"/hover popups rather than
  literal text substitution).
- Editing operations (`rename_function`, `rename_variable`, `set_comment`, etc.) return the rizin
  command string they ran, so the caller can replay the same command on the background decompiler
  pipe (`forward_edit`) to keep the two rizin instances' analysis state in sync.
