# Tests on AWS Device Farm

The tests that CI's target jobs run, built for phones and run on real ones in
[AWS Device Farm](https://aws.amazon.com/device-farm/):

- Android: built for `aarch64-linux-android`, on a Google Pixel 9 (Mali GPU) and
  a Samsung Galaxy S25 (Adreno GPU), both on Android 16, with the render suite
  on Vulkan.
- iOS: built for `aarch64-apple-ios`, on an Apple iPhone 17 Pro on iOS 26, with
  the render suite on Metal and its API validation.

`schedule.sh` lists the phones. The runs take the account's flat-rate devices,
which maplibre-native's CI shares, and wait while its runs have them all: the
account has five Android devices and one iPhone. Each platform's directory holds
the test spec, `test-spec.yml`, and the scripts that run the tests on the
phones.

## How a run works

1. `mise run device-farm:package <platform>` builds the test package into
   `target/device-farm/<platform>/`.
2. `schedule.sh` uploads the package with the platform's test spec, and
   schedules them. Once the run finished, `collect.sh` downloads each phone's
   test spec output and artifacts.
3. On each phone's test host, the platform's `run-tests` runs the tests on the
   phone. The artifacts hold a JUnit report and the tests' output
   (`test-output.tar.gz`): the render suite's report, which shows its failed
   tests' images, and the images that other tests write. A test that fails gets
   one more try, which the report shows as flaky when it passes.

### Android

The package, `tests.zip`, holds the tests archived by cargo-nextest, the test
host for the phones' ABI (`tools/android-test-host`), the workspace's files,
Linux x64 `cargo-nextest` and `devices-cli`, and `run-tests.sh`. `schedule.sh`
schedules it as a web app test, the kind of test that needs no app.

On a Linux test host, `run-tests.sh` unpacks the workspace and assigns the phone
in `devices-cli`. cargo-nextest then runs the archive's tests with the
`device-farm` profile (`.config/nextest.toml`), each test on the phone through
`tools/android-test-host/runner.sh` and `devices-cli`, just as on an emulator.
Each test runs in a Java VM's process, as an app's code does, through the same
test host and runner that the emulator's runs use.

### iOS

The package is an app, `app.ipa`: the test host (`tools/ios-test-host`) with the
tests built as dylibs and the workspace's files. `tests.zip` holds
`run-tests.py` and `tests.json`, which lists the tests. `schedule.sh` schedules
them as an app test, for which Device Farm signs the app, which is signed ad
hoc, and installs it on the phone.

On a macOS test host, `run-tests.py` launches the app with `devicectl` once for
each test binary, whose harness runs all of the binary's tests in the app's
process, as `cargo test` runs them: an app runs in one process at a time, so a
launch per test would take hours. It leaves out the tests that nextest's
`target` profile leaves out on Android, and reads each test's result from the
harness's JSON output. The tests that fail, or that a crash or a timeout left
without a result, get their one more try together, in one more launch.

## In CI

CI's `prepare-android-device-tests` and `prepare-ios-device-tests` jobs build
the packages when the `android` and `ios` units run, without credentials. Three
workflows run them, so that no GitHub runner waits for the phones while a run is
queued or in progress:

- `device-test` starts with the CI run, waits for each platform's job, and
  schedules its package once the job has built it, while the rest of CI runs. It
  reports each run on the commit as pending, as the `device-test / android` and
  `device-test / ios` statuses, which link to the run in the AWS console
  (`console-url.sh`), and names the run for the commit and the platform. A newer
  commit of a pull request stops the runs of the commit before it. When the plan
  leaves a platform's tests out, it reports its status as passed, since merging
  into `main` requires it.
- `device-farm-watch`, one at a time for every run, waits for the runs, and
  starts `device-farm-finish` for each that finished (`watch.sh`). An hourly
  schedule starts it too, in case one failed.
- `device-farm-finish` reports the run's result on its commit, linking to itself
  rather than to the AWS console, which needs a login to the account. It has
  each phone's output and artifacts (`collect.sh`), and a job without
  credentials summarizes each phone's JUnit report (`summary.py`).

Pull requests from forks get a run safely because the orchestrator always reads
`.config/device-farm.toml` and `android.yml` from `main` and never unpacks or
executes the package outside AWS Device Farm.

[maplibre/infra](https://github.com/maplibre/infra) manages the AWS side: the
`device-farm/` Lambda orchestrator and the `opentofu/device-farm` stack (the
`mln-rs` Device Farm project, Lambdas, queue, table, and results distribution).

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
