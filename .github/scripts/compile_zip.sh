#!/bin/env bash
# shellcheck disable=SC2035

if [ -z "$GITHUB_WORKSPACE" ]; then
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
	"mainfiles/system/bin"
	"mainfiles/libs"
	"mainfiles/META-INF"
	"mainfiles/service.sh"
	"mainfiles/post-fs-data.sh"
	"mainfiles/action.sh"
	"mainfiles/cleanup.sh"
	"mainfiles/uninstall.sh"
	"mainfiles/module.prop"
    "mainfiles/module.banner.jpg"
	"mainfiles/azenithApplist.json"
    "mainfiles/AZenith.apk"
)

# Version info
version="$(cat version)"
version_type="$(cat version_type | tr -d '\n\r ')" # Hapus spasi/newline agar presisi
version_code="$(git rev-list HEAD --count)"
release_code="$(git rev-list HEAD --count)-$(git rev-parse --short HEAD)-$version_type"
sed -i "s/version=.*/version=$version ($release_code)/" mainfiles/module.prop
sed -i "s/versionCode=.*/versionCode=$version_code/" mainfiles/module.prop

# Set Profile Folder untuk Rust berdasarkan version_type
RUST_PROFILE="release"
if [ "$version_type" == "experimental" ]; then
    RUST_PROFILE="debug"
fi
echo "Using Rust build profile: $RUST_PROFILE"

mkdir -p mainfiles/libs/arm64-v8a
mkdir -p mainfiles/libs/armeabi-v7a
mkdir -p mainfiles/system/bin

# The C tree is gone; everything now comes out of one cargo target dir.
# Cargo rejects `.` in `[[bin]]` names, so the bins are dot-free and get
# their `sys.azenith-*` names here.
#
#   azenith-daemon  -> sys.azenith-service          (the only real binary)
#   preloadbin      -> sys.azenith-preloadbin      (separate process, plan Q3)
#   rianixia-thermalcore -> sys.azenith-rianixiathermalcore
#
# profilesettings / utilityconf / preferredtweaks are no longer shipped: the
# daemon links them in and dispatches in-process (plan Q1, Option A).

collect_rust_bins() {
	abi_target="$1"   # aarch64-linux-android | armv7-linux-androideabi
	out_dir="$2"      # mainfiles/libs/arm64-v8a | mainfiles/libs/armeabi-v7a
	src="target/$abi_target/$RUST_PROFILE"

	for pair in \
		"azenith-daemon:sys.azenith-service" \
		"preloadbin:sys.azenith-preloadbin" \
		"rianixia-thermalcore:sys.azenith-rianixiathermalcore"; do
		bin="${pair%%:*}"
		dest="${pair##*:}"
		if [ -f "$src/$bin" ]; then
			cp "$src/$bin" "$out_dir/$dest"
		else
			echo "ERROR: missing $src/$bin"
			exit 1
		fi
	done
}

collect_rust_bins aarch64-linux-android mainfiles/libs/arm64-v8a
collect_rust_bins armv7-linux-androideabi mainfiles/libs/armeabi-v7a

# The daemon hard-exits at boot if its compiled-in MODULE_VERSION does not
# equal module.prop's `version=`. That check fires on the *device*, hours later
# and with no useful log, so verify the agreement here first.
#
# The version is baked in by crates/azenith-common/build.rs, which derives it
# from the same version/version_type/git inputs used above. If the two ever
# disagree, the build step and the packaging step ran against different trees.
# upx_compress.sh may have packed these binaries, and `strings` cannot see
# through UPX: it finds zero occurrences of the version in a packed file even
# though the string is still in .rodata. Unpack a scratch copy first rather
# than gating on a string that only exists pre-compression.
built_version() {
	bin="$1"
	# upx -q still prints its banner on stdout in 5.x, so silence both streams:
	# anything leaking here lands in the caller's command substitution.
	if upx -t -q "$bin" >/dev/null 2>&1; then
		tmp="$(mktemp -d)"
		cp "$bin" "$tmp/probe"
		upx -d -q "$tmp/probe" >/dev/null 2>&1
		strings "$tmp/probe" | grep -oE "$version \([0-9]+-[0-9a-f]+-[^)]*\)" | head -n1
		rm -rf "$tmp"
	else
		strings "$bin" | grep -oE "$version \([0-9]+-[0-9a-f]+-[^)]*\)" | head -n1
	fi
}

packaged="$version ($release_code)"
for abi in arm64-v8a armeabi-v7a; do
	bin="mainfiles/libs/$abi/sys.azenith-service"
	built="$(built_version "$bin")"
	if [ "$built" != "$packaged" ]; then
		echo "ERROR: $bin was built with version '$built' but module.prop says '$packaged'"
		exit 1
	fi
done
echo ">>> version agreement OK: $version ($release_code)"

# Other Files
cp azenithApplist.json mainfiles/
cp LICENSE mainfiles/ 2>/dev/null
cp NOTICE.md mainfiles/ 2>/dev/null

# Copy Manager APK
APK_PATH=$(find manager/app/build/outputs/apk/release -name "*.apk" | head -n 1)
APK_PATH_DEBUG=$(find manager/app/build/outputs/apk/debug -name "*.apk" | head -n 1)
if [ -n "$APK_PATH" ]; then
    cp "$APK_PATH" "mainfiles/AZenith.apk"
    echo "APK found at $APK_PATH and copied to mainfiles successfully."
elif [ -n "$APK_PATH_DEBUG" ]; then
    cp "$APK_PATH_DEBUG" "mainfiles/AZenith.apk"
    echo "APK found at $APK_PATH_DEBUG and copied to mainfiles successfully."
else
    echo "ERROR: No APK found!"
    exit 1
fi

# The APK carries its own versionCode, and the Manager shows "update
# available" by comparing it against the module's. A stale APK therefore makes
# the app report an older build than the module it shipped inside — which is
# exactly what a human sees as "my app says 1864 but latest is 1867". The
# Gradle step runs before this one in CI, so a mismatch means the APK was not
# rebuilt for this version: fail instead of shipping a misleading package.
#
# Read the expected value back out of module.prop rather than reusing
# $version_code: that is the value the Manager actually compares against, and
# it is already final at this point in the script.
MODULE_VERSION_CODE=$(sed -n 's/^versionCode=//p' mainfiles/module.prop)
# aapt2 is the only reliable reader here. `strings` on the manifest does not
# surface the attribute — AndroidManifest.xml is binary XML, and the value is
# an int, not a string pool entry.
AAPT2=$(ls "${ANDROID_SDK_ROOT:-$ANDROID_HOME}/build-tools/"*/aapt2 2>/dev/null | sort -V | tail -n 1)
if [ -z "$AAPT2" ]; then
    echo "WARNING: aapt2 not found; skipping the APK versionCode check"
else
    APK_VERSION_CODE=$("$AAPT2" dump badging mainfiles/AZenith.apk 2>/dev/null |
        sed -n "s/.*versionCode='\([0-9]*\)'.*/\1/p")
    if [ -z "$APK_VERSION_CODE" ]; then
        echo "ERROR: aapt2 could not read a versionCode out of the APK"
        exit 1
    elif [ "$APK_VERSION_CODE" != "$MODULE_VERSION_CODE" ]; then
        echo "ERROR: APK versionCode=$APK_VERSION_CODE but module versionCode=$MODULE_VERSION_CODE"
        echo "       Rebuild the Manager (./gradlew assembleDebug) before zipping."
        exit 1
    else
        echo "APK versionCode matches module versionCode=$MODULE_VERSION_CODE"
    fi
fi

# Parse version info to module prop
zipName="AZenith-$version-$release_code.zip"
echo "zipName=$zipName" >>"$GITHUB_OUTPUT"
artifactName="${zipName%.zip}"
echo "artifactName=$artifactName" >>"$GITHUB_OUTPUT"

# Generate sha256sum for integrity checkup
for file in "${need_integrity[@]}"; do
	bash .github/scripts/generatesha256.sh "$file"
done

# Zip the file
cd ./mainfiles || {
	echo "Unable to cd to ./mainfiles" >&2
	exit 1
}

zip -r9 ../"$zipName" * -x *placeholder* *.map .shellcheckrc
zip -z ../"$zipName" <<EOF
$version-$release_code
Build Date $(date +"%a %b %d %H:%M:%S %Z %Y")
EOF
