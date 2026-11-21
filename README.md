# lvim-fuzzy-plugin-preview

The preview plugin of the lvim tools (`lvc`, `lfz`, the Neovim picker). It **decides** which engine shows a file and
**computes** the answer; the tool only **draws** it (`lvim_fuzzy_ui::provided`). Everything it does is configuration.

A `process` plugin (JSON lines); build with `cargo build --release`, install with `lvim-fuzzy-plugin install`.

## Types and engines

A **type** is a group of files; its `engine` shows it. A group matches by `ext`, `name`, `mime` (`"image/*"`) and / or
`kind` (`dir`, `binary`, `text`) — every given key must hold. The most specific group wins (`ext` / `name` / `mime`
before `kind` only, then `priority`). A file no group matches is passed back to the tool.

```toml
[config.types.images]          # by MIME type
mime   = ["image/*"]
engine = "host"                # the tool's own picture preview

[config.types.rust]            # by extension, to your own engine
ext    = ["rs", "toml"]
engine = "bat"

[config.engines.bat]           # an engine of your own: a command, {} = the file
kind    = "command"
command = "bat --color=always --style=numbers --paging=never --line-range=:500 {}"
ansi    = true
```

Built-in engines: `dir`, `table`, `archive`, `binary`, `nvim` (a real Neovim), `builtin` (the tool's highlighter),
`host` (pass). A program an engine needs that is not on the PATH is said under the preview (red `✗`), and the file is
shown plain.

## What is configured now

`lvim-fuzzy-cli preview --list` prints the type → engine map in effect for lfz (the defaults, the shared file and lfz's own
layer), best group first; a group whose engine runs a program is marked when that program is not installed.

## Who overrides what

defaults (`plugin.toml`) < `~/.config/lvim-plugins/lvim-fuzzy-plugin-preview.toml` (every tool) < the tool's own config:
lfz `[plugins.lvim-fuzzy-plugin-preview]`, lvc `[plugins.lvim-fuzzy-plugin-preview]`, the picker `setup{ plugins = { … } }`. Each tool
may map the types differently.
