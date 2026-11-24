# Android tests on AWS Device Farm

The tests that CI's target jobs run, built for `aarch64-linux-android` and run
on real phones in [AWS Device Farm](https://aws.amazon.com/device-farm/): a
Google Pixel 9 (Mali GPU) and a Samsung Galaxy S25 (Adreno GPU), both on Android
16, with the render suite on Vulkan. The phones are listed in `schedule.sh`.

## How a run works

1. `mise -E target-android run device-farm:package` builds the test package,
   `target/device-farm/android-tests.zip`. It holds the tests archived by
   cargo-nextest, the test host for the phones' ABI (`tools/android-test-host`),
   the workspace's files, a Linux x64 cargo-nextest, and `run-tests.sh` and
   `runner.sh`.
2. `schedule.sh` uploads the package with the test spec, `android.yml`. It
   schedules them as a web app test, the kind of test that needs no app, waits
   for the run, and downloads each phone's test spec output and artifacts.
3. On each phone's test host, `run-tests.sh` copies the workspace to the phone.
   cargo-nextest then runs the archive's tests with the `device-farm` profile
   (`.config/nextest.toml`), each test on the phone through `runner.sh` and
   `adb shell`, as devices-cli runs them on an emulator. The artifacts hold
   nextest's JUnit report and the tests' output (`test-output.tar.gz`): the
   render suite's report, which shows its failed tests' images, and the images
   that other tests write. A test that fails gets one more try, which nextest
   reports as flaky when it passes.

Each test runs in a Java VM's process, as an app's code does, through the test
host that the emulator's runs use too (`tools/android-test-host`). `runner.sh`
copies files with `adb push` rather than through devices-cli, since Device
Farm's `adb` garbles what `adb shell` reads from stdin.

## In CI

CI's `device-farm` job builds the package when the `android` unit runs, without
credentials. Once the CI run completes, the `device-farm` workflow
(`.github/workflows/device-farm.yml`) runs it and reports the result on the
commit as the `device-farm / android` status, and a job without credentials
summarizes each phone's JUnit report (`summary.py`). That workflow runs on
`workflow_run`, from the default branch, so pull requests from forks get a run
too: the package may come from a fork, so the workflow, which assumes its AWS
role through the `workflow-run` environment that only `main` can use, never runs
or unpacks it. A newer commit of a pull request stops the run of the commit
before it.

[maplibre/infra](https://github.com/maplibre/infra)'s `opentofu/github-actions`
stack manages the AWS side: the `mln-rs` Device Farm project, the role, the
`workflow-run` environment and its variables. The role is shared with
maplibre-native's `workflow_run` jobs.

## From your machine

With AWS credentials for MapLibre's account:

```sh
mise -E target-android run device-farm:package
mise run device-farm:run
```

The results land in `target/device-farm/results/<phone>/`. The phones are the
account's flat-rate devices, which maplibre-native's CI shares.
