#!/bin/sh
# Build the plugin and install it: the program goes to ~/.local/bin, the plugin (its plugin.toml, with `exec` pointing at that program) to the
# shared plugins directory ~/.config/lvim-plugins/lvim-fuzzy-plugin-preview/ — every lvim fuzzy tool (lvc, lfz, the picker) finds it there.
set -e
cd "$(dirname "$0")/.."
name=lvim-fuzzy-plugin-preview
cargo build --release
bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
dest="${LVIM_PLUGINS_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/lvim-plugins}/$name"
mkdir -p "$bin" "$dest"
cp "${CARGO_TARGET_DIR:-target}/release/$name" "$bin/$name.new"
mv "$bin/$name.new" "$bin/$name"
sed "s|^exec .*|exec        = \"$bin/$name\"|" plugin.toml > "$dest/plugin.toml.new"
mv "$dest/plugin.toml.new" "$dest/plugin.toml"
echo "installed: $bin/$name, $dest/plugin.toml"
