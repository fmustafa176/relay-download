# Relay

Relay is a Rust desktop app for resumable HTTP downloads and torrents. Direct downloads keep partial files across sessions. The Torrents button opens qBittorrent using a dedicated Relay profile.

## Install on Arch Linux

```sh
sudo pacman -S --needed base-devel rust lld gtk4 curl git qbittorrent
./install.sh
```

Run **Relay** from the app menu or run `~/.local/bin/relay`. The translucent window uses the desktop compositor.

## Update

Install from a Git clone of the project's GitHub repository. The installer picks up its `origin` URL. For a local source archive, set `RELAY_GITHUB_REPO` to the repository's HTTPS URL when running the installer.

Relay checks for updates on launch. Click **Update now** to build and install the current GitHub revision, then restart Relay. From a terminal, run `~/.local/lib/relay/update --apply`. Updates use the same build dependencies as installation.
