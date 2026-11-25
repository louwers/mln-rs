# Tests on AWS Device Farm

The tests that CI's target jobs run, built for phones and run on real ones in
[AWS Device Farm](https://aws.amazon.com/device-farm/):

- Android: built for `aarch64-linux-android`, on a Google Pixel 9 (Mali GPU) and
  a Samsung Galaxy S25 (Adreno GPU), both on Android 16, with the render suite
  on Vulkan.
- iOS: built for `aarch64-apple-ios`, on an Apple iPhone 17 Pro on iOS 26, with
  the render suite on Metal and its API validation.

`.config/device-farm.toml` configures each platform's run: its phones, test
type, test spec and timeout. The runs take the account's flat-rate devices,
which maplibre-native's CI shares, and wait while its runs have them all: the
account has five Android devices and one iPhone. Each platform's directory holds
its test spec, `test-spec.yml`, and the scripts that run the tests on the
phones.

## How a run works

1. `mise run device-farm:package <platform>` builds the test package into
   `target/device-farm/<platform>/`.
2. The package is uploaded with the platform's test spec and scheduled, by the
   orchestrator in CI, or by `schedule.sh` from your machine, where `collect.sh`
   downloads each phone's test spec output and artifacts once the run finished.
3. On each phone's test host, the platform's `run-tests` runs the tests on the
   phone. The artifacts hold a JUnit report and the tests' output
   (`test-output.tar.gz`): the render suite's report, which shows its failed
   tests' images, and the images that other tests write. A test that fails gets
   one more try, which the report shows as flaky when it passes.

### Android

The package, `tests.zip`, holds the tests archived by cargo-nextest, the test
host for the phones' ABI (`tools/android-test-host`), the workspace's files,
Linux x64 `cargo-nextest` and `devices-cli`, and `run-tests.sh`. It runs as a
web app test, the kind of test that needs no app.

On a Linux test host, `run-tests.sh` unpacks the workspace and assigns the phone
in `devices-cli`. cargo-nextest then runs the archive's tests with the
`device-farm` profile (`.config/nextest.toml`), each test on the phone through
`tools/android-test-host/runner.sh` and `devices-cli`, just as on an emulator.
Each test runs in a Java VM's process, as an app's code does, through the same
test host and runner that the emulator's runs use.

### iOS

The package is an app, `app.ipa`: the test host (`tools/ios-test-host`) with the
tests built as dylibs and the workspace's files. `tests.zip` holds
`run-tests.py` and `tests.json`, which lists the tests. They run as an app test,
for which Device Farm signs the app, which is signed ad hoc, and installs it on
the phone.

On a macOS test host, `run-tests.py` launches the app with `devicectl` once for
each test binary, whose harness runs all of the binary's tests in the app's
process, as `cargo test` runs them: an app runs in one process at a time, so a
launch per test would take hours. It leaves out the tests that nextest's
`target` profile leaves out on Android, and reads each test's result from the
harness's JSON output. The tests that fail, or that a crash or a timeout left
without a result, get their one more try together, in one more launch.

## In CI

The phones run `main`'s commits. Pull requests run the tests on the Android
emulator and the iOS simulator, in CI's `target` jobs, which `main` runs too.

CI's `prepare-<platform>-device-tests` jobs build the packages, without AWS
credentials, and upload them as the `<platform>-device-tests` artifacts. A pull
request builds a platform's package only when it changes the packaging, to check
that it still builds.

The `maplibre-device-farm` GitHub App runs the packages, without holding a
GitHub runner while the phones run. Its orchestrator, which
[maplibre/infra](https://github.com/maplibre/infra) deploys with the `mln-rs`
Device Farm project, reads `.config/device-farm.toml` and the test specs from
`main`, and never unpacks or runs a package:

- Once a prepare job has built `main`'s package, it schedules the package on the
  platform's phones and reports the run on the commit as the
  `device-test / <platform>` check run, which links to the run in the AWS
  console.
- When the run finishes, it publishes each phone's output and artifacts and
  completes the check run with a summary of each phone's JUnit report. GitHub's
  Re-run button on the check run runs the package again.
- It passes a pull request's check runs as not needed, since merging requires
  `device-test / android`.

## From your machine

With AWS credentials for MapLibre's account, Android's package in the
target-android environment:

```sh
mise -E target-android run device-farm:package android
mise run device-farm:run android
```

and iOS's on a Mac with Xcode:

```sh
mise run device-farm:package ios
mise run device-farm:run ios
```

The results land in `target/device-farm/results/<phone>/`.
