# Android test host

Runs the tests built for Android in a Java VM's process, as an app's code runs,
so that what needs the Java VM works in a test: HTTP (`HttpURLConnection`) and
the locale-sensitive expressions (`android.icu`). A test executable that
`adb shell` runs has no Java VM.

`launcher.sh` starts Android's runtime on the device with `app_process`, as
Android starts an app's process, and runs `TestHost.java`. Its native half,
`test-host.c`, loads the test executable into the process and calls its `main`,
which the target-android environment's builds export. The library finds the
process's Java VM (`mln_style::android::java_vm`). Each test still runs in a
process of its own, so nextest runs them in parallel.

- `build.sh` builds the test host for an ABI, in the target-android environment:
  the dex with its JDK and the build-tools' d8, and the native half with the
  NDK.
- `runner.sh` is the environment's Cargo runner for Android
  (`.config/mise/config.target-android.toml`). It runs each test on this
  checkout's emulator, or a device assigned to it, through devices-cli, which
  copies the executable, the test host and the test's package to the device. It
  builds the test host into `target/android-test-host` the first time, and again
  when its sources change.
- AWS Device Farm's runs use the same test host and `runner.sh`, with
  `ANDROID_TEST_HOST` set to the prebuilt test host in the test package
  (`tools/device-farm`).
