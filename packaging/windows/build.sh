#!/usr/bin/env bash
# Cross-build the Windows binaries, installer and portable zip from Linux.
#
#   packaging/windows/build.sh [version]
#
# Needs: rustup target x86_64-pc-windows-gnu, mingw-w64, nsis, zip.
# Everything it produces lands in dist/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Default to the workspace version, so there is one place to bump.
VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)}"
TARGET="x86_64-pc-windows-gnu"
BIN="$ROOT/target/$TARGET/release"
STAGE="$ROOT/target/windows-stage"
DIST="$ROOT/dist"

missing=()
command -v x86_64-w64-mingw32-gcc >/dev/null || missing+=("mingw-w64")
command -v makensis >/dev/null || missing+=("nsis")
command -v zip >/dev/null || missing+=("zip")
rustup target list --installed | grep -qx "$TARGET" || missing+=("rustup target add $TARGET")
if [ ${#missing[@]} -ne 0 ]; then
  printf 'Missing: %s\n' "${missing[*]}" >&2
  exit 1
fi

echo "Building $TARGET binaries"
cargo build --release --target "$TARGET" -p springen-cli -p springen-app

rm -rf "$STAGE"
mkdir -p "$STAGE" "$DIST"
cp "$BIN/springen.exe" "$BIN/springen-app.exe" "$STAGE/"
cp "$ROOT/packaging/windows/springen.ico" "$STAGE/"
cp "$ROOT/packaging/windows/README.txt" "$STAGE/"
cp "$ROOT/LICENSE" "$STAGE/LICENSE.txt"
x86_64-w64-mingw32-strip "$STAGE/springen.exe" "$STAGE/springen-app.exe"

# Portable zip: the same files, no installer, nothing written outside the folder.
ZIP="$DIST/springen-$VERSION-windows-x64-portable.zip"
rm -f "$ZIP"
(cd "$STAGE" && zip -q -9 "$ZIP" springen.exe springen-app.exe springen.ico README.txt LICENSE.txt)

# Installer.
EXE="$DIST/springen-$VERSION-windows-x64-setup.exe"
makensis -V2 \
  "-DVERSION=$VERSION" \
  "-DSRCDIR=$STAGE" \
  "-DOUTFILE=$EXE" \
  "$ROOT/packaging/windows/springen.nsi"

echo
for f in "$ZIP" "$EXE"; do
  printf '%-52s %8.1f MB\n' "$(basename "$f")" "$(echo "scale=2; $(stat -c%s "$f") / 1048576" | bc)"
done
echo "in $DIST"
