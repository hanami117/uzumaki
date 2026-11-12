# Hooks

Drop an executable named `on-song-change` or `on-state-change` into `~/.config/uzumaki/hooks/`
(`$XDG_CONFIG_HOME/uzumaki/hooks/`; on macOS too) and Uzumaki runs it, with the window and with `uzu`
alike. No config file, no shell: the script is started directly and gets its data as environment
variables: `UZUMAKI_EVENT` (`song-change` or `state-change`), `UZUMAKI_STATE` (`playing`, `paused`,
`stopped`, and `loading` on a song change that hasn't started yet), `UZUMAKI_TITLE`, `UZUMAKI_ARTIST`,
`UZUMAKI_ALBUM`, `UZUMAKI_DURATION` (seconds), `UZUMAKI_ID`, `UZUMAKI_PREV_ID` (the song before, on a
song change), `UZUMAKI_SOURCE` (`apple` or `local`), `UZUMAKI_LOCAL` (`1` or `0`), `UZUMAKI_FILE` (the
path of a local song) and `UZUMAKI_COVER` (a cover image file once it is cached, waiting up to 2 s;
empty if it isn't).

A hook must be yours and not writable by group or others (`chmod 755`), or it is skipped and the
reason goes to the log. It has no stdin, its output goes to the log, and it gets 10 s (then SIGTERM, then
SIGKILL, for its whole process group). One run per hook at a time: while it runs, only the newest
event waits, so skipping through songs never piles up runs. Hooks are off in test instances.

```sh
#!/bin/sh
# ~/.config/uzumaki/hooks/on-song-change
notify-send -i "${UZUMAKI_COVER:-audio-x-generic}" "$UZUMAKI_TITLE" "$UZUMAKI_ARTIST — $UZUMAKI_ALBUM"
# macOS: osascript -e 'display notification (system attribute "UZUMAKI_ARTIST") with title (system attribute "UZUMAKI_TITLE")'
```

