#!/usr/bin/env bash
# Builds the macOS part of a release on this Mac and adds it to the release that CI made for the tag.
#
#   packaging/macos/release.sh <tag> [--repo owner/name] [--dry-run]
#
# The Linux files (tarball, AppImage, .zsync, notes.md, latest.json) are built by GitHub Actions on
# tag push, and the workflow makes the (draft) release. This script then:
#   1. checks that the tag exists, HEAD is exactly the tag and the tree is clean;
#   2. checks that the release exists (draft or not): if not, CI has to run first;
#   3. downloads the tarball and the AppImage files from the release (they are never rebuilt:
#      their sha256 is pinned by the AUR) and runs `packaging/release.sh --assets` on them. That is
#      the same command the workflow's macos job runs, so the names and checksums are the same, and
#      build.sh does the signing (ad hoc, plus the castLabs VMP check; EVS_PYTHON as in build.sh);
#   4. uploads, with --clobber, the 5 files the Mac adds, then checks GitHub's digests:
#        Uzumaki-<release>.dmg  (+ .sha256)   the disk image
#        Uzumaki-macOS.dmg      (+ .sha256)   the same bytes under the stable name
#        SHA256SUMS                           replaces CI's 8-file one with the 12-file one
#      notes.md and latest.json stay as CI made them (latest.json links the stable names and has no
#      checksums, so the DMG doesn't change it).
#
# --dry-run builds and prints what it would upload; it uploads nothing.
# On a published release that already has the DMG it stops: never replace a file after publishing.
# gh runs as hanami117 without switching accounts (GH_TOKEN from `gh auth token -u hanami117`,
# unless GH_TOKEN is already set).
set -euo pipefail

cd "$(dirname "$0")/../.."
say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf 'macos/release.sh: %s\n' "$*" >&2; exit 1; }

usage="usage: packaging/macos/release.sh <tag> [--repo owner/name] [--dry-run]"
repo=hanami117/uzumaki
tag=
dry=0
while (($#)); do
    case "$1" in
        --repo) [[ -n ${2:-} ]] || die "--repo needs owner/name"; repo=$2; shift ;;
        --dry-run) dry=1 ;;
        -h|--help) echo "$usage"; exit ;;
        -*) die "unknown option '$1'. $usage" ;;
        *) [[ -z $tag ]] || die "one tag only. $usage"; tag=$1 ;;
    esac
    shift
done
[[ -n $tag ]] || die "$usage"
[[ $repo =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] || die "--repo must look like owner/name (got '$repo')"
[[ $tag == v* ]] || die "the tag should look like v0.1.1 (got '$tag')"
rel=${tag#v}

[[ $(uname -s) == Darwin ]] || die "run this on a Mac"
command -v gh >/dev/null || die "gh is missing"

# gh as hanami117, without `gh auth switch`.
if [[ -z ${GH_TOKEN:-} ]]; then
    GH_TOKEN=$(gh auth token -u hanami117 2>/dev/null) || die "no gh login for hanami117 (gh auth login), or set GH_TOKEN"
    export GH_TOKEN
fi

# --- Checks -------------------------------------------------------------------------------------
git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "no tag $tag here (git fetch --tags, or tag the release commit)"
[[ $(git rev-parse "$tag^{commit}") == $(git rev-parse HEAD) ]] ||
    die "HEAD isn't $tag (the tag is $(git rev-parse --short "$tag^{commit}"), HEAD is $(git rev-parse --short HEAD)): git checkout $tag"
[[ -z $(git status --porcelain) ]] || die "the working tree isn't clean:
$(git status --short)"

# The release must exist already: CI makes it (a draft is fine).
draft=$(gh release view "$tag" -R "$repo" --json isDraft --jq .isDraft 2>/dev/null) ||
    die "no release $tag in $repo. CI has to run first (push the tag and wait for the draft)."
names=$(gh release view "$tag" -R "$repo" --json assets --jq '.assets[].name')
say "Release $tag in $repo ($([[ $draft == true ]] && echo draft || echo published)), $(wc -l <<<"$names" | tr -d ' ') assets"

# The files the Mac reuses, as the workflow's macos job takes them from the Linux artifact.
need=("uzumaki-$rel.tar.gz" "Uzumaki-$rel-x86_64.AppImage" Uzumaki-x86_64.AppImage Uzumaki-x86_64.AppImage.zsync)
for f in "${need[@]}"; do
    grep -qxF "$f" <<<"$names" || die "the release has no $f: CI isn't finished, or it built another version"
done
if [[ $draft != true ]] && grep -qxF "Uzumaki-$rel.dmg" <<<"$names"; then
    if ((dry)); then
        warn "the published release already has its DMG; a real run would stop here"
    else
        die "$tag is published and already has Uzumaki-$rel.dmg: never replace a file after publishing"
    fi
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# --- Build --------------------------------------------------------------------------------------
say "Downloading the Linux files from the release"
mkdir "$work/assets"
args=()
for f in "${need[@]}"; do args+=(-p "$f"); done
gh release download "$tag" -R "$repo" -D "$work/assets" "${args[@]}"

# Checks the tag again, builds the DMG, reuses the tarball, writes the .sha256 files and SHA256SUMS.
say "Building the macOS release (packaging/release.sh --assets)"
packaging/release.sh --assets "$work/assets"

out=${CARGO_TARGET_DIR:-target}/release-$rel
up=("Uzumaki-$rel.dmg" "Uzumaki-$rel.dmg.sha256" Uzumaki-macOS.dmg Uzumaki-macOS.dmg.sha256 SHA256SUMS)
for f in "${up[@]}"; do [[ -f $out/$f ]] || die "$out/$f wasn't made"; done
# All 12 files are there and match the sums that go to the release.
(cd "$out" && shasum -a 256 -c SHA256SUMS >/dev/null) || die "SHA256SUMS doesn't match the files in $out"
[[ $(wc -l <"$out/SHA256SUMS" | tr -d ' ') == 5 ]] || die "SHA256SUMS should list 5 files (tarball, 2 AppImages, 2 DMGs)"

say "To upload to $repo release $tag (--clobber):"
(cd "$out" && ls -l "${up[@]}")
if ((dry)); then
    say "Dry run: nothing uploaded."
    exit 0
fi

# --- Upload -------------------------------------------------------------------------------------
say "Uploading"
(cd "$out" && gh release upload "$tag" -R "$repo" --clobber "${up[@]}")

# GitHub's digests are the local files'.
say "Checking GitHub's digests"
remote=$(gh release view "$tag" -R "$repo" --json assets \
    --jq '.assets[] | (.digest | sub("^sha256:"; "")) + "  " + .name' | sort -k2)
for f in "${up[@]}"; do
    local_sum=$(shasum -a 256 "$out/$f" | cut -d' ' -f1)
    grep -qxF "$local_sum  $f" <<<"$remote" || die "GitHub's digest of $f isn't the local file's"
done
n=$(gh release view "$tag" -R "$repo" --json assets --jq '.assets | length')
say "Done: $tag now has $n assets (12 are needed to publish)."
