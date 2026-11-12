# Draft of the Homebrew Cask for the tap hanami117/homebrew-uzumaki (file Casks/uzumaki.rb there).
# Users install it with: brew install --cask hanami117/uzumaki/uzumaki
#
# Fill in at release time (packaging/release.sh prints both values):
#   - version   the release version, without the v. From build 2 of a version on (the BUILD
#               file; a rebuild without code changes) it is "<version>,<build>", e.g. "0.1.0,2":
#               Homebrew orders it after "0.1.0", and the url below picks the build's files.
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

  # Build 1: tag v0.1.0, Uzumaki-0.1.0.dmg. Build 2 and up: tag v0.1.0-2, Uzumaki-0.1.0-2.dmg.
  url "https://github.com/hanami117/uzumaki/releases/download/v#{version.csv.second ? "#{version.csv.first}-#{version.csv.second}" : version}/Uzumaki-#{version.csv.second ? "#{version.csv.first}-#{version.csv.second}" : version}.dmg"
  name "Uzumaki"
  desc "Native desktop client for Apple Music"
  homepage "https://getuzumaki.app/"

  # Tags are v<version> or v<version>-<build>; the build becomes Homebrew's ",<build>".
  livecheck do
    url :url
    regex(/^v?(\d+(?:\.\d+)+)(?:-(\d+))?$/i)
    strategy :github_latest do |json, regex|
      match = json["tag_name"]&.match(regex)
      next if match.blank?

      match[2] ? "#{match[1]},#{match[2]}" : match[1]
    end
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
