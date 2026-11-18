# Relay

Relay is a Linux download manager written in Rust. It saves partial HTTP downloads so you can resume them later. Torrents use a dedicated qBittorrent profile.

## Install on Arch Linux

```sh
sudo pacman -S --needed base-devel rust lld gtk4 curl git qbittorrent
git clone https://github.com/fmustafa176/relay-download.git
cd relay-download
./install.sh
```

Open **Relay** from your app menu or run `~/.local/bin/relay`. The translucent window uses your desktop compositor.

## Headless mode

Close the Relay window before running direct downloads from the terminal. Both modes use the same queue and partial files.

```sh
~/.local/bin/relay --headless add https://example.com/file.zip ~/Downloads
~/.local/bin/relay --headless list
~/.local/bin/relay --headless resume TRANSFER_ID
~/.local/bin/relay --headless run
```

`resume` accepts an optional replacement URL. `run` resumes all paused transfers and exits when the queue is idle. Press Ctrl+C to stop and save resumable progress.

With `qbittorrent-nox` installed, use `~/.local/bin/relay --headless torrents` to run qBittorrent without a display. It accepts an optional magnet link, torrent URL, or torrent file. Close desktop qBittorrent using the Relay profile before starting the headless torrent process.

## Update

Relay checks GitHub on launch. Use the gear menu to check again or install an update, then restart Relay. From a terminal, run `~/.local/lib/relay/update --apply`. Updates build from the same GitHub repository you cloned.
