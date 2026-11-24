#!/system/bin/sh
# shellcheck shell=sh
# Runs a test executable built for Android in a Java VM's process on the
# device: app_process starts Android's runtime, as it does for an app's
# process, with TestHost, which loads the executable and calls its main.
# $ANDROID_TEST_HOST is the directory that holds the test host (build.sh).
#
# Usage: launcher.sh <executable> [arguments]
CLASSPATH=$ANDROID_TEST_HOST/test-host.jar exec app_process /system/bin org.maplibre.mlnrs.TestHost \
    "$ANDROID_TEST_HOST/libmln_test_host.so" "$@"
