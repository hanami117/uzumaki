Uzumaki for Windows (preview)
=============================

A desktop player for Apple Music and for your own music files.
Please report what goes wrong (see the end of this file). The programs are not signed yet.

Needs Windows 10 (version 1809 or newer) or Windows 11, 64-bit (x64).
Apple Music needs an Apple Music subscription; your own files play without one.


Install
-------
With the installer (Uzumaki-...-windows-x64-setup.exe): run it. Windows will say "Windows
protected your PC" (SmartScreen), because the program is not signed: click "More info",
then "Run anyway". Setup installs Uzumaki for you only, never asks for administrator rights,
adds a Start menu entry and puts uzu (the command line) on your PATH. A new version is
installed the same way, over the old one. Uninstall in Windows Settings, Apps, Installed
apps: your settings and sign-in are kept unless you say otherwise.

From the zip: unpack it somewhere of your own, for example C:\Uzumaki (not inside Program
Files), and double-click uzumaki.exe in the Uzumaki folder. Keep the folder whole: the
bridge folder beside uzumaki.exe is the playback engine. If SmartScreen has no "Run anyway":
right-click the zip before unpacking, Properties, tick "Unblock", OK.

First start: choose Apple Music or "local files only". For Apple Music, sign in once in the
window that opens (it is Apple's own web player). The first start also downloads Google's
Widevine component (a few MB), which Apple Music needs; give it a minute.


What should work
----------------
- The window, your library, playlists, search, albums, artists, Now Playing with lyrics,
  the queue, Settings.
- Apple Music playback (256 kbps AAC, as on the web player).
- Your own music files (Settings, Account or the first-run choice): FLAC, MP3, AAC/M4A,
  ALAC, Ogg Vorbis, WAV.
- The keyboard's media keys, headset buttons and Windows' media overlay (with the volume
  keys, and in Quick Settings): title, artist, cover, play/pause, next, previous.
- The notification-area icon (the tray, next to the clock; maybe behind the ^ arrow): left
  click shows the window, right click has Play/Pause, Next, Previous, Quit. With "Close to
  tray" on in Settings, closing the window keeps the music playing.
- A notification when the song changes while Uzumaki is not in front (Settings, General).
- Start with Windows: Settings, General, "Start Uzumaki when I log in" (and "Start in the
  background").
- Only one Uzumaki at a time: starting it again shows the window of the running one.
- Discord: Settings, General, "Show on Discord" (needs the Discord desktop app).
- uzu, the command line and terminal player. In PowerShell or Windows Terminal:
      uzu              the interactive screens (queue, playlists, search, lyrics)
      uzu status       what plays now
      uzu toggle       play/pause          uzu next / uzu prev
      uzu search daft punk                 uzu play 1
      uzu quit         quits Uzumaki
  If Uzumaki isn't running, uzu starts it hidden. (From the zip, uzu is not on your PATH:
  run .\uzu in Uzumaki's folder.)


Updates
-------
Once a day Uzumaki asks getuzumaki.app whether a new version is out and shows a notice.
Nothing is downloaded by itself. There is no Windows download on the site yet, so the
notice's button opens the release's page. Switch the check off in Settings, General.


What does not work on Windows yet
---------------------------------
- AirPlay speakers (the AirPlay button is hidden).
- Script hooks (on-song-change, on-state-change).
- Animated album artwork needs ffmpeg.exe on your PATH; without it covers stay still.
- Windows' volume mixer and Task Manager list the playback engine as "Electron".
- Lossless and Dolby Atmos: Apple's web player does not offer them.
- The programs are not signed: SmartScreen warns at every new version.


If Apple Music does not play
----------------------------
Uzumaki plays Apple Music through castLabs Electron and Google's Widevine, the same way a
browser does. If songs do not start, or the sign-in window says "Something went wrong":
1. Wait a minute after the very first start (the Widevine component downloads), then quit
   Uzumaki (tray icon, Quit) and start it again.
2. Check the date and time of the PC.
3. Send the two log files named below.
Your own music files do not depend on any of this.


Where Uzumaki keeps its files
-----------------------------
    Settings, uzu.toml     %APPDATA%\Uzumaki\
    Apple Music sign-in    %LOCALAPPDATA%\Uzumaki\Bridge\
    Caches (covers, ...)   %LOCALAPPDATA%\Uzumaki\Cache\
    Logs                   %LOCALAPPDATA%\Uzumaki\Logs\uzumaki.log  and  bridge.log

(Paste a path into the address bar of Explorer to open it.)
To remove Uzumaki completely: uninstall it and answer Yes when it asks about your settings
(from the zip: quit it, delete the folder you unpacked and the two Uzumaki folders above).


How to report a problem
-----------------------
Please send:
1. What you did, what you expected and what happened.
2. A screenshot of the window (Win+Shift+S).
3. The two log files in %LOCALAPPDATA%\Uzumaki\Logs\
   Do NOT send the Bridge folder: it holds your Apple Music sign-in.
4. Your Windows version (Win+R, winver) and, for a blank or broken window, your graphics
   card.
5. The version: Settings, About (or   uzu --version   in PowerShell).

For a window that stays blank or crashes at start, run this in PowerShell in Uzumaki's
folder and send the file gpu.txt it makes:
    $env:RUNE_GPU_LOG = "1"; .\uzumaki.exe 2>&1 | Out-File gpu.txt

Report at https://github.com/hanami117/uzumaki/issues


Uzumaki is free software under the GPL, version 3 or later. It is not affiliated with Apple.
Source: https://github.com/hanami117/uzumaki
The playback engine is castLabs Electron (MIT; Chromium's licences are in
bridge\node_modules\electron\dist\LICENSES.chromium.html). Widevine is Google's.
