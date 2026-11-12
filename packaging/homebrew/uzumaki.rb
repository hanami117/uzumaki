# Draft of the Homebrew Cask for the tap hanami117/homebrew-uzumaki (file Casks/uzumaki.rb there).
# Users install it with: brew install --cask hanami117/uzumaki/uzumaki
#
# Fill in at release time (packaging/release.sh prints both values):
#   - version   the release version, without the v
#   - sha256    the DMG's checksum (Uzumaki-<version>.dmg.sha256 on the release)
# Test: brew audit --cask --strict hanami117/uzumaki/uzumaki; brew install --cask …; brew uninstall --zap …
#
# homebrew/cask proper needs a notarized app; the tap doesn't. The app is signed ad hoc, so the
# first launch is refused by Gatekeeper (the caveats below say what to do).
# The app's update notice sees the cask's receipt (Caskroom/uzumaki/.metadata) and says
# "update with your package manager" instead of offering the DMG (app/src/update.rs).
cask "uzumaki" do
  version "0.1.0"
  # FILL AT RELEASE: the DMG's sha256 (packaging/release.sh prints it).
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/hanami117/uzumaki/releases/download/v#{version}/Uzumaki-#{version}.dmg"
  name "Uzumaki"
  desc "Native desktop client for Apple Music"
  homepage "https://getuzumaki.app/"

  livecheck do
    url :url
    strategy :github_latest
  end

  # The app's own update notice only points here; it never replaces itself.
  auto_updates false
  depends_on arch: :arm64
  depends_on macos: ">= :ventura"

  app "Uzumaki.app"
  binary "#{appdir}/Uzumaki.app/Contents/MacOS/uzu"

  uninstall quit: "io.github.hanami117.uzumaki"

  # Settings and hooks live in ~/.config/uzumaki on macOS too; caches in ~/.cache/uzumaki; state
  # (update check, uzu) in ~/.local/state/uzumaki; playlist pictures in ~/.local/share/uzumaki; the
  # Apple Music sign-in (Electron's profile) in ~/Library/Application Support/uzumaki/bridge; the
  # log in ~/Library/Logs/Uzumaki. Not removable here: the login's Keychain item (Electron's
  # "Safe Storage"), which Keychain Access can delete.
  zap trash: [
    "~/.cache/uzumaki",
    "~/.config/uzumaki",
    "~/.local/share/uzumaki",
    "~/.local/state/uzumaki",
    "~/Library/Application Support/uzumaki",
    "~/Library/Logs/Uzumaki",
    "~/Library/Preferences/io.github.hanami117.uzumaki.plist",
    "~/Library/Saved Application State/io.github.hanami117.uzumaki.savedState",
  ]

  caveats <<~EOS
    Uzumaki is not notarized. On first launch macOS refuses it: open
      System Settings → Privacy & Security
    and click "Open Anyway". Or run:
      xattr -dr com.apple.quarantine #{appdir}/Uzumaki.app
    An Apple Music subscription is needed for playback.
  EOS
end
