# vizin

A terminal disassembler/decompiler with **vim/nvim keybindings** — Ghidra's power in a modal CLI.

vizin drives [rizin](https://rizin.re) plus the [rz-ghidra](https://github.com/rizinorg/rz-ghidra)
plugin (Ghidra's actual C++ decompiler), and puts a modal, keyboard-only interface on top of it:
a function sidebar, a disassembly listing, a live Ghidra decompiler pane with syntax highlighting,
strings/imports/exports/segments panels, an xrefs popup, and a hex editor with byte patching.

```
┌ Functions (157) ─┐┌ Listing — main ──────────────┐┌ Decompiler (Ghidra) — main ─┐
│  3560 main       ││   3560 endbr64               ││ uint64_t main(int argc,     │
│  54f0 entry0     ││   3564 push rbp              ││                char **argv) │
│  5520 fcn.005520 ││   359d call fcn.000181e0     ││ {                           │
│  ...             ││   ...                        ││   ...                       │
└──────────────────┘└──────────────────────────────┘└─────────────────────────────┘
 NORMAL  test_ls  [x86/64 RO]   main   0x3560
```

## Requirements

- `rizin` (0.8+) on `PATH`
- `rz-ghidra` plugin installed (for the decompiler — vizin degrades gracefully without it)
- Rust toolchain to build

## Build & run

```sh
cargo build --release
./target/release/vizin /bin/ls          # read-only
./target/release/vizin -w ./target      # write mode (enables patching)
./target/release/vizin -p session.rzdb /bin/ls   # load a saved project
```

On open, vizin runs full analysis (`aaa`) once, then drops you at `main` (or `entry0`).
Pass `--no-analysis` to skip it.

## Keybindings

Modal, like vim. `:` opens the command line, `/` searches, `Esc`/`q` backs out.

### Motion
| Key | Action |
|-----|--------|
| `h j k l` / arrows | left / down / up / right |
| `{count}j` | repeat motion (e.g. `12j`) |
| `gg` / `G` | top / bottom |
| `Ctrl-d` / `Ctrl-u` | half page down / up |
| `Ctrl-f` / `Ctrl-b` | page down / up |
| `H` / `M` / `L` | screen top / middle / bottom |
| `zz` `zt` `zb` | center / top / bottom cursor in screen |
| `w` `b` `e` | word next / prev / end (decompiler only) |
| `f{char}` `F{char}` | find char forward / back on current line (decompiler only) |
| `Space` | toggle listing ⇄ decompiler for the current function |
| `Tab` | cycle focus: sidebar → listing → decompiler |

### Navigation
| Key | Action |
|-----|--------|
| `Enter` / `gd` | follow the call/jump/symbol under the cursor |
| `Ctrl-o` / `Ctrl-i` | jump back / forward (jumplist) |
| `x` / `X` | xrefs **to** / **from** here (popup) |
| `[{` / `]}` | jump to previous / next `{` `}` block (decompiler only) |
| `%` | jump to matching bracket `{}()[]` (decompiler only) |
| `*` / `#` | search forward / backward for word under cursor |
| `/pat` `n` `N` | search current view, next / previous |
| `K` | hover popup: value under cursor as dec/hex/oct/bin/char/string, plus any comment |
| `?` | show this keybinding cheatsheet |

### Views / Panes
| Key | Action |
|-----|--------|
| `Ctrl-w h` / `l` / `w` | focus sidebar / main / cycle focus |
| `Ctrl-w h` | focus sidebar (left pane) |
| `Ctrl-w l` | focus main pane (right pane) |
| `:fn` | focus the function list (type `/` there to filter) |
| `:str` `:imp` `:exp` `:seg` | strings / imports / exports / segments panels |
| `:hex` | hex view |
| `q` | close popup/panel |

### Editing (persisted via rizin projects)
| Key | Action |
|-----|--------|
| `r` | rename the function / variable / label under the cursor |
| `;` | add / edit a comment at the current address |
| `i` | (hex view) enter insert mode; type hex digits, `Esc` commits the patch |
| `:w [file.rzdb]` | save renames/comments to a project file |
| `:oo+` | reopen the file in write mode (enable patching mid-session) |
| `:q` / `:q!` | quit (`:q` warns on unsaved changes) |

Renames and comments in the **decompiler** pane work too: the cursor maps to
addresses and symbols through Ghidra's own annotations, so `gd`, `x`, `r`, and `;`
act on whatever pseudo-C token you're sitting on.

## How it works

```
vizin (Rust + ratatui)
  └─ pipe.rs — spawn `rizin -q0`, NUL-delimited command protocol
      └─ backend.rs — typed API (serde) over rizin JSON commands
          └─ rizin  ── aflj / pdj / axtj / izzj …  (analysis, disasm, xrefs, symbols)
                    └─ rz-ghidra ── pdgj  (Ghidra decompiler → C + per-char annotations)
```

The decompiler pane consumes `pdgj`'s annotation stream (`offset`, `syntax_highlight`,
`function_name`, `local_variable`, `global_variable`, …) to color the code and to map
every character back to an address — which is what makes listing⇄decompiler cursor sync
and go-to-definition inside decompiled code possible.

## License

MIT
