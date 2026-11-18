#!/bin/sh
set -eu
cd -- "$(dirname -- "$0")"

prefix=${RELAY_PREFIX:-"$HOME/.local"}
desktop_dir=${RELAY_DESKTOP_DIR:-"${XDG_DATA_HOME:-$HOME/.local/share}/applications"}
revision=${RELAY_REVISION:-$(git rev-parse HEAD 2>/dev/null || printf development)}
remote=${RELAY_GITHUB_REPO:-$(git remote get-url origin 2>/dev/null || true)}
case "$remote" in
  git@github.com:*) remote="https://github.com/${remote#git@github.com:}" ;;
esac
case "$remote" in
  ""|https://github.com/*/*) ;;
  *) echo "Set RELAY_GITHUB_REPO to the HTTPS URL of your GitHub repository." >&2; exit 1 ;;
esac

if [ "${1:-}" = "--binary" ]; then
    binary=$2
    test -x "$binary"
else
    export RELAY_REVISION="$revision"
    export CARGO_TARGET_DIR="$PWD/target"
    cargo build --release --locked
    binary=target/release/relay
fi

mkdir -p "$prefix/bin" "$prefix/lib/relay" "$desktop_dir"
staged=$(mktemp "$prefix/bin/.relay.XXXXXX")
trap 'rm -f -- "$staged"' EXIT
install -m755 "$binary" "$staged"
install -m755 scripts/update "$prefix/lib/relay/update"
if [ -n "$remote" ]; then
    printf '%s\n' "$remote" > "$prefix/lib/relay/github-url"
fi
mv -f -- "$staged" "$prefix/bin/relay"
exec_path=$(printf '%s' "$prefix/bin/relay" | sed 's/[\\"$`]/\\&/g; s/%/%%/g')
cat > "$desktop_dir/io.github.Relay.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Relay
Comment=Download files and torrents
Exec="$exec_path" %u
Icon=folder-download
Terminal=false
Categories=Network;FileTransfer;
DESKTOP
printf 'Installed Relay at %s\n' "$prefix/bin/relay"
