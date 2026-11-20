# Discord Rich Presence ("Show on Discord")

Uzumaki can show what you are listening to on your Discord profile, like Spotify does: a "Listening to
Uzumaki" card with the song, the artist, the cover, a progress bar and an "Open in Apple Music" button.
It is off by default: Settings → Notifications → **Show on Discord**.

## How it works

- Uzumaki talks to the **Discord desktop app running on the same computer**, through its local socket
  (`discord-ipc-0` … `discord-ipc-9` in `$XDG_RUNTIME_DIR`, `$TMPDIR` or `/tmp`, including the Flatpak and Snap
  folders). Uzumaki makes no network connection for this. Discord itself shows the card to others, and fetches
  the cover from Apple's image server when it draws it.
- Nothing runs unless the switch is on: no thread, no socket. When it is on, one worker thread connects (and
  looks again every 30 seconds while Discord isn't running), sends the card when the song, the play state or the
  position jumps (seek) change, and clears it when you pause (with "Hide when paused"), stop, switch it off or
  quit. It never waits for Discord on the window's thread.
- Web Discord (in a browser) and Discord on another computer have no such socket: nothing shows.

What the card holds: the song as the title line, the artist as the second line, the album cover (Apple's, as a
link; for a local file the Uzumaki icon instead), the album name when you hover the cover, the Uzumaki icon as the
small picture, the time elapsed and the song's length, and a button that opens the song on music.apple.com.
Local files show title and artist only.

| Setting | Default | |
|---|---|---|
| Show on Discord | off | the main switch |
| Show artwork | on | off: the Uzumaki icon replaces the cover; no cover link goes to Discord |
| Show buttons | on | the "Open in Apple Music" button (Apple Music songs only) |
| Hide when paused | on | off: the card stays while paused, without the progress bar |
| Hide song title | off | "Listening to music" and the artist; no album, no button |

They live in `settings.json` (`~/.config/uzumaki/`, `~/Library/Application Support/…` as the other settings):

```json
"discord": { "enabled": false, "artwork": true, "buttons": true, "hide_when_paused": true,
             "hide_title": false, "client_id": null }
```

## The Discord Application (what the project owner does once)

Discord needs an *Application* to attach the card to; its name is what people read after "Listening to".

1. Go to <https://discord.com/developers/applications> (sign in with your Discord account) and press
   **New Application**. Name it **Uzumaki** (this name appears on the card) and accept the terms.
2. On **General Information**, copy the **Application ID**. This is the `client_id`.
3. Optional but nice: on General Information, set the App Icon (the Uzumaki swirl, `assets/icons/hicolor/512x512/apps/io.github.hanami117.uzumaki.png`).
4. Open **Rich Presence → Art Assets** and upload the Uzumaki icon (at least 512×512, ideally 1024×1024) with the
   asset key **`uzumaki`** (exactly). It is the small picture on every card and the big one for local files and
   when artwork is off. Discord can take a few minutes before a new asset shows.
5. The Application ID is in `app/src/discord/mod.rs`, the `CLIENT_ID` constant (Uzumaki's real id, the same on
   every platform). To try another app without a build, set `"client_id": "<the id>"` inside `discord` in
   `settings.json`.

No OAuth, redirect URL, bot, token or secret is needed or used; the Application ID is public. Don't add any.

## Trying it

With the Discord desktop app open and the switch on, play a song: your profile (click your avatar → look at
the status, or ask a friend) shows "Listening to Uzumaki". Discord's *Settings → Activity Privacy → Share your
detected activity* must be on, or it hides all cards. Your own client doesn't draw the buttons on your own
card; others see them.

Troubleshooting: the log (`~/Library/Logs/Uzumaki/uzumaki.log` on macOS, the terminal / journal on Linux) has
one `Discord:` line when it connects and one when the connection is lost or Discord isn't there.
"Discord closed the handshake" is usually a wrong `client_id`.

## Not done yet

- Windows (the named pipe `\\.\pipe\discord-ipc-N`; `discord/ipc.rs` has the names). The app does not build
  for Windows yet anyway.
- Discord on Flatpak/Snap with Uzumaki *outside* the sandbox works through the folders above; Uzumaki *inside* a
  sandbox would need access to Discord's socket folder.
