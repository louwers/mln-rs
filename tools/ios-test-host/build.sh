#!/usr/bin/env bash
# Builds the test host into an app bundle for iOS devices: main.m as the app's
# executable, and Info.plist with the SDK and the oldest iOS it runs on.
# `mise run device-farm:package ios` adds the tests and the workspace, and
# signs it. Needs Xcode.
#
# Usage: build.sh <app directory>
set -euo pipefail

app=$1
here=$(cd "$(dirname "$0")" && pwd)
# The oldest iOS that AWS Device Farm's current test host runs tests on.
minimum=17.0

mkdir -p "$app"
xcrun --sdk iphoneos clang -arch arm64 -mios-version-min="$minimum" -fobjc-arc -O2 -Wall -Wextra -Werror \
    -framework UIKit "$here/main.m" -o "$app/MlnRsTestHost"
cp "$here/Info.plist" "$app/"
plutil -replace MinimumOSVersion -string "$minimum" "$app/Info.plist"
plutil -replace DTSDKName -string "iphoneos$(xcrun --sdk iphoneos --show-sdk-version)" "$app/Info.plist"
