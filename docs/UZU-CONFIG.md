# The uzu config file

`uzu` is the command line and terminal player of Uzumaki. You can change how it looks and
behaves with one small file. You do not need the file. Without it, `uzu` works as it always did.

This file is only for `uzu`. The app has its own shortcuts in Settings → Keyboard.
They are kept apart on purpose.

## Where the file is

```
~/.config/uzumaki/uzu.toml
```

If `XDG_CONFIG_HOME` is set, the file is `$XDG_CONFIG_HOME/uzumaki/uzu.toml` instead.

`uzu` reads the file when it starts. It never writes to it. You are the only one who edits it.
What `uzu` remembers by itself (your sort choices) is in a different file, `uzu.json`, in
`~/.local/state/uzumaki/`. If you change the config, restart `uzu` to see the change.

## Start from the full example

```sh
mkdir -p ~/.config/uzumaki
uzu config --default > ~/.config/uzumaki/uzu.toml
```

This prints every setting with a comment. Every line is commented out and shows its default.
Remove the `# ` in front of a line to change it.

## Check the file

```sh
uzu config            # where the file is, and what is wrong in it
uzu config --path     # only the path
uzu config --default  # the full commented file
```

`uzu config` exits with 1 when it finds a problem. That makes it easy to use in a script.

## What happens when something is wrong

`uzu` never stops because of a mistake in the file. It skips the bad line and uses the rest.
Each problem is a warning with the line number and, when it can, a hint:

```
line 4: unknown key 'acent' in [theme]; did you mean 'accent'?
```

In the terminal player, the first warning shows in the status line for a few seconds.
The line also says how many more there are. `uzu config` lists them all.

## The format of the file

The file is TOML. The part `uzu` reads is small:

```toml
# A comment.
[section]
name = "text"
number = 5
switch = true
list = ["one", "two"]
```

- Text needs quotes.
- A list can run over several lines.
- A line that `uzu` cannot read is a warning. It does not stop the rest.

## [keys]: change the keys

Each line is an action and its keys.

```toml
[keys]
next = ["n", "ctrl+n"]   # two keys for next song
quit = []                # no key for quit
down = ["j", "down", "ctrl+n"]
```

A list **replaces** the default keys of that action. Keys you do not mention keep their
defaults. If you want to keep an old key, write it in the list too.

A key is one character, like `n`, `N` or `?`. Letters are case sensitive. Or it is a name:
`space`, `enter`, `esc`, `tab`, `backspace`, `delete`, `insert`, `up`, `down`, `left`, `right`,
`home`, `end`, `pageup`, `pagedown`, `f1` to `f12`. Put `ctrl+`, `alt+` or `shift+` in front:
`ctrl+n`, `alt+x`.

Things to know:

- `Ctrl+C` always quits. You cannot bind it to anything else.
- While you type (a filter, a search), every key is text. Your keys do not apply there.
- The sort menu and the help box read their own keys. Your keys do not apply there.
- The arrow keys Left and Right, Tab and `h` keep their meaning on the screens where they
  switch panes or seek. If you bind one of them, your binding wins and you get a warning.
- If you give one key to two actions, the first one keeps it and you get a warning.
- If you take the default key of an action you did not change, you get a warning. That action
  loses the key.
- When you change any key, a Ctrl or Alt key that you did not bind does nothing.
- The help (`?`) and the hints at the bottom show your keys.

The actions and their default keys:

| Action           | Default            | What it does                                     |
|------------------|--------------------|--------------------------------------------------|
| `play_pause`     | `space`            | play or pause                                    |
| `next`           | `n`                | next song                                        |
| `prev`           | `p`                | previous song                                    |
| `seek_forward`   | `f`                | seek forward 10 seconds                          |
| `seek_back`      | `b`                | seek back 10 seconds                             |
| `volume_up`      | `+`, `=`           | volume up 5%                                     |
| `volume_down`    | `-`                | volume down 5%                                   |
| `shuffle`        | `s`                | shuffle on or off                                |
| `repeat`         | `r`                | repeat: off, all, one                            |
| `goto_queue`     | `1`                | the Queue screen                                 |
| `goto_playlists` | `2`                | the Playlists screen                             |
| `goto_search`    | `3`                | the Search screen                                |
| `goto_albums`    | `4`                | the Albums screen                                |
| `goto_lyrics`    | `5`                | the Lyrics screen                                |
| `cover`          | `i`                | show or hide the cover                           |
| `view`           | `F`                | view: normal, showcase, solo                     |
| `lyrics`         | `l`                | in solo: show or hide the lyrics                 |
| `words`          | `w`                | lyrics: word by word or line by line             |
| `help`           | `?`                | the help                                         |
| `quit`           | `q`                | quit                                             |
| `up`             | `k`, `up`          | move up                                          |
| `down`           | `j`, `down`        | move down                                        |
| `page_up`        | `pageup`           | a page up                                        |
| `page_down`      | `pagedown`         | a page down                                      |
| `top`            | `g`, `home`        | first row                                        |
| `bottom`         | `G`, `end`         | last row                                         |
| `select`         | `enter`            | play or open the selected row                    |
| `back`           | `esc`              | clear the filter, step back, or quit             |
| `filter`         | `/`                | filter (on Search: type a new query)             |
| `sort`           | `,`                | open the sort menu                               |
| `playing`        | `o`                | select the playing song                          |
| `remove`         | `d`, `delete`      | remove the selected song from Up Next            |
| `move_down`      | `J`                | move the selected song down in Up Next           |
| `move_up`        | `K`                | move the selected song up in Up Next             |
| `play_all`       | `P`                | play the whole playlist or album                 |
| `reload`         | `R`                | reload the list                                  |
| `add`            | `a`                | Search: add the result to the queue              |
| `play_next`      | `N`                | Search: play the result next                     |

The `lyrics` key is `l`. On the Playlists and Albums screens, `l` also switches panes. If you
change `lyrics`, the old `l` stops switching panes there too. The arrow keys still work.

## [theme]: change the colours

```toml
[theme]
palette = "default"
accent = "#ff7a8a"
number = "green"
album = "cyan"
time = "magenta"
paused = "yellow"
respect_no_color = true
```

- `palette` is `"default"` (coral and the ANSI colours), `"terminal"` (only the 16 colours of
  your terminal theme) or `"mono"` (no colour).
- The five colours are put on top of the palette. Order in the file does not matter.
- A colour is `"#rrggbb"` or `"#rgb"`, a name, a number from 0 to 255, or `"none"`.
  `"none"` is the terminal's own text colour.
- The names are `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `gray`,
  `white`, `dark-gray`, `light-red`, `light-green`, `light-yellow`, `light-blue`,
  `light-magenta`, `light-cyan` and `coral`.
- If `NO_COLOR` is set, `uzu` uses no colour. Set `respect_no_color = false` to ignore it.

What each colour paints: `accent` is the playing song, the current tab, the key names in the
help and the sung lyrics. `number` is the `#` column. `album` is the Album column.
`time` is the Time column. `paused` is the word "Paused".

The theme is for the terminal player. The plain text from commands like `uzu status` does
not use it.

## [format]: a line for a status bar

```toml
[format]
status_format = "{artist} — {title} [{elapsed}/{duration}]"
status_idle = ""
```

Then run:

```sh
uzu status --line
```

It prints one line. You can also give the text on the command line:

```sh
uzu status --format "{icon} {title:30}"
```

The names you can use:

| Name          | Value                                      |
|---------------|--------------------------------------------|
| `{artist}`    | the artist                                 |
| `{title}`     | the song                                   |
| `{album}`     | the album                                  |
| `{elapsed}`   | time played, like `1:12`                   |
| `{duration}`  | length of the song, like `3:59`            |
| `{remaining}` | time left, like `2:47`                     |
| `{percent}`   | how far, from 0 to 100 (no `%` sign)       |
| `{state}`     | `playing`, `paused`, `loading`, `stopped`  |
| `{icon}`      | `▶`, `‖`, `…` or `■`                       |
| `{volume}`    | from 0 to 100 (no `%` sign)                |
| `{shuffle}`   | `on` or `off`                              |
| `{repeat}`    | `off`, `all` or `one`                      |
| `{source}`    | the list the music plays from              |
| `{index}`     | the place of the song in that list         |
| `{count}`     | the number of songs in that list           |
| `{queued}`    | songs added with Play Next or Add to Queue |

- `{title:20}` cuts the title to 20 columns and adds `…`.
- `{{` and `}}` print a brace.
- A name that does not exist prints nothing. You get a warning, with a hint.
- `status_idle` is printed when nothing plays. The default is empty.
  Many bars hide a module that prints nothing.
- `uzu status --line` never starts Uzumaki. If the app is not running, it prints the idle text.
  This is on purpose, so a bar that asks every second does not start the app again.
- If you have no `status_format`, the line is `{artist} — {title}`.

Examples:

```jsonc
// waybar
"custom/uzu": { "exec": "uzu status --line", "interval": 2 }
```

```toml
# starship
[custom.uzu]
command = "uzu status --line"
when = "pgrep -x uzumaki"
```

```sh
# tmux
set -g status-right '#(uzu status --line)'
```

## [layout]: where things go

```toml
[layout]
start_screen = "queue"
start_view = "normal"
lyrics = "auto"
artist_width = 3
title_width = 5
album_width = 3
show_artist = true
show_album = true
```

- `start_screen` is `"queue"`, `"playlists"`, `"search"`, `"albums"` or `"lyrics"`. A number
  from 1 to 5 also works.
- `start_view` is the view of the queue screen: `"normal"`, `"showcase"` or `"solo"`.
  The `UZUMAKI_VIEW` variable wins over the file. If `start_screen` is not the queue,
  the screen you asked for wins and the view is normal.
- `lyrics` is where solo puts the lyrics. `"auto"` puts them beside the cover when there is
  room, and under the song when there is not. `"beside"` never goes under: a narrow terminal
  shows solo without lyrics. `"under"` always goes under.
- `artist_width`, `title_width` and `album_width` are shares of the width of the song table.
  They are whole numbers from 1 to 20. With 3, 5 and 3, the title gets 5 parts out of 11.
- `show_artist = false` and `show_album = false` hide those columns. A narrow terminal still
  hides them by itself, as before.

## [defaults]: how things start

```toml
[defaults]
sort_playlists = "original"
sort_albums = "original"
sort_songs = "original"
sort_search = "original"
notifications = true
```

- `sort_*` is how a list starts. Playlists: `"name"` or `"song count"`. Albums: `"name"`,
  `"artist"` or `"year"`. Songs and search: `"title"`, `"artist"`, `"album"` or `"duration"`.
  Add `desc` for the other way, like `"year desc"`. `"original"` is the order from Uzumaki.
  `"newest first"` is the original order reversed.
- A sort you pick with `,` is remembered in `uzu.json` and wins over the default.
  `uzu` never writes the defaults to `uzu.json`. If you pick the original order again,
  the default comes back the next time you start `uzu`.
- `notifications = false` hides the short messages in the status line, like "Paused" and
  "Cover: on". Errors from Uzumaki, the "went away" note and problems in this file still show.

## A themed example

```toml
# ~/.config/uzumaki/uzu.toml: a cool blue theme, vim-like keys, a bar line.

[theme]
accent = "#88c0d0"
number = "#81a1c1"
album  = "#8fbcbb"
time   = "#b48ead"
paused = "#ebcb8b"

[keys]
next         = ["n", "ctrl+n"]
prev         = ["p", "ctrl+p"]
down         = ["j", "down", "ctrl+d"]
up           = ["k", "up", "ctrl+u"]
play_pause   = ["space", "ctrl+space"]
select       = ["enter", "ctrl+j"]
quit         = ["q", "ctrl+q"]

[format]
status_format = "{icon} {artist} — {title:30} [{elapsed}/{duration}]"
status_idle   = "♪"

[layout]
start_screen = "playlists"
lyrics       = "under"
title_width  = 8
show_album   = false

[defaults]
sort_playlists = "name"
sort_albums    = "year desc"
```

## Not in this version

- **Hot reload.** `uzu` reads the file once, when it starts. Reloading while it runs is a
  planned next step.
- The colours of plain command output (`uzu status`, `uzu playlists`) are fixed.
