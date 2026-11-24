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
2. `schedule.sh` uploads the package with the test spec, `android.yml`, and
   schedules them as a web app test, the kind of test that needs no app. Once
   the run finished, `collect.sh` downloads each phone's test spec output and
   artifacts.
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
credentials. Three workflows run it, so that no GitHub runner waits for the
phones while a run is queued or in progress:

- `device-farm` schedules the package once the CI run completes, and reports it
  on the commit as pending, as the `device-farm / android` status. It names the
  run for the commit. A newer commit of a pull request stops the run of the
  commit before it.
- `device-farm-watch`, one at a time for every run, waits for the runs, and
  starts `device-farm-finish` for each that finished (`watch.sh`). An hourly
  schedule starts it too, in case one failed.
- `device-farm-finish` reports the run's result on its commit, with each phone's
  output and artifacts (`collect.sh`), and a job without credentials summarizes
  each phone's JUnit report (`summary.py`).

`device-farm` runs on `workflow_run`, from the default branch, so pull requests
from forks get a run too: the package may come from a fork, so the workflows,
which assume their AWS role through the `workflow-run` environment that only
`main` can use, never run or unpack it.

[maplibre/infra](https://github.com/maplibre/infra)'s `opentofu/github-actions`
stack manages the AWS side: the `mln-rs` Device Farm project, the `workflow-run`
environment and its variables, and the role, `GitHubActionsWorkflowRun`, which
every maplibre repository's workflows can assume to run tests on Device Farm.

## From your machine

With AWS credentials for MapLibre's account:

```sh
mise -E target-android run device-farm:package
mise run device-farm:run
```

The results land in `target/device-farm/results/<phone>/`. The phones are the
account's flat-rate devices, which maplibre-native's CI shares.
