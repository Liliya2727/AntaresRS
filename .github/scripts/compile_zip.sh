#!/bin/env bash
# shellcheck disable=SC2035

set -eo pipefail

if [ -z "${GITHUB_WORKSPACE:-}" ]; then
	echo "This script should only run on GitHub action!" >&2
	exit 1
fi

# Make sure we're on right directory
cd "$GITHUB_WORKSPACE" || {
	echo "Unable to cd to GITHUB_WORKSPACE" >&2
	exit 1
}

# Put critical files and folders here
need_integrity=(
	"mainfiles/libs"
	"mainfiles/system/bin"
	"mainfiles/META-INF"
	"mainfiles/service.sh"
	"mainfiles/post-fs-data.sh"
	"mainfiles/action.sh"
	"mainfiles/cleanup.sh"
	"mainfiles/uninstall.sh"
	"mainfiles/module.prop"
	"mainfiles/module.banner.jpg"
	"mainfiles/gamelist.txt"
)

# Version info
version="$(cat version)"
version_code="$(git rev-list HEAD --count)"
release_code="$(git rev-parse --short HEAD)-Release"
sed -i "s/version=.*/version=$version ($release_code)/" mainfiles/module.prop
sed -i "s/versionCode=.*/versionCode=$version_code/" mainfiles/module.prop

# Compile Gamelist (1 package name per line, read directly by the daemon)
cp gamelist.txt mainfiles/gamelist.txt

# Copy the binaries for every supported ABI
for abi in arm64-v8a armeabi-v7a; do
	case "$abi" in
		arm64-v8a)   rust_target="aarch64-linux-android" ;;
		armeabi-v7a) rust_target="armv7-linux-androideabi" ;;
	esac

	out="mainfiles/libs/$abi"
	mkdir -p "$out"

	# The daemon is installed as sys.azenith-service: sys.azenith-profilesettings
	# refuses to run unless the caller cmdline contains "sys.azenith-service".
	# (cargo cannot emit a binary name with a dot, hence the rename here)
	cp "archdaemon/target/$rust_target/release/sys_azenithrs-service" "$out/sys.azenith-service"
	cp "binprofiles/target/$rust_target/release/azenith-profilesettings" "$out/sys.azenith-profilesettings"
	cp "binutils/target/$rust_target/release/azenith-utilityconf" "$out/sys.azenith-utilityconf"
	cp "binpreferenced/target/$rust_target/release/azenith-preferencedtweaks" "$out/sys.azenith-preferencedtweaks"

	# vmtouch clone, ndk-build output
	cp "preloadbin/libs/$abi/sys.azenith-preloadbin" "$out/sys.azenith-preloadbin"

	echo "Staged $abi: $(ls "$out" | tr '\n' ' ')"
done

# Other files
cp LICENSE mainfiles/ 2>/dev/null || true

# Parse version info to module prop
zipName="AntaresRS-$version-$release_code.zip"
echo "zipName=$zipName" >>"$GITHUB_OUTPUT"
artifactName="${zipName%.zip}"
echo "artifactName=$artifactName" >>"$GITHUB_OUTPUT"

# Generate sha256sum for integrity checkup
for file in "${need_integrity[@]}"; do
	if [ ! -e "$file" ]; then
		echo "ERROR: $file not found, cannot generate checksum" >&2
		exit 1
	fi
	bash .github/scripts/generatesha256.sh "$file"
done

# Zip the file
cd ./mainfiles || {
	echo "Unable to cd to ./mainfiles" >&2
	exit 1
}

zip -r9 "../$zipName" * -x *placeholder* *.map .shellcheckrc
zip -z "../$zipName" <<EOF
$version-$release_code
Build Date $(date +"%a %b %d %H:%M:%S %Z %Y")
EOF
