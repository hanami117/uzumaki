-- The only table: how many update checks came in per UTC day and (version, os, arch, channel).
-- One row per combination, no per-request rows, no addresses or ids of any kind.
CREATE TABLE IF NOT EXISTS checks (
  day     TEXT    NOT NULL,          -- '2026-11-12' (UTC)
  version TEXT    NOT NULL,          -- '0.2.0'
  os      TEXT    NOT NULL,          -- linux | macos | windows
  arch    TEXT    NOT NULL,          -- x86_64 | aarch64 | unknown
  channel TEXT    NOT NULL,          -- dmg | homebrew | appimage | aur | system | install-sh | source | unknown
  n       INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, version, os, arch, channel)
) WITHOUT ROWID;
