# Android tests on AWS Device Farm

The tests that CI's target jobs run, built for `aarch64-linux-android` and run
on real phones in [AWS Device Farm](https://aws.amazon.com/device-farm/): a
Google Pixel 9 (Mali GPU) and a Samsung Galaxy S25 (Adreno GPU), both on Android
16, with the render suite on Vulkan. The phones and run settings are configured
in `.config/device-farm.toml`.

## How a run works

1. `mise -E target-android run device-farm:package` builds the test package,
   `target/device-farm/android-tests.zip`. It holds the tests archived by
   cargo-nextest, the test host for the phones' ABI (`tools/android-test-host`),
   the workspace's files, Linux x64 `cargo-nextest` and `devices-cli`, and
   `run-tests.sh`.
2. The package is uploaded with the test spec, `android.yml`, and scheduled as a
   web app test, the kind of test that needs no app (by the Lambda orchestrator
   in CI, or by `schedule.sh` and `collect.sh` locally).
3. On each phone's test host, `run-tests.sh` unpacks the workspace and assigns
   the phone in `devices-cli`. cargo-nextest then runs the archive's tests with
   the `device-farm` profile (`.config/nextest.toml`), each test on the phone
   through `tools/android-test-host/runner.sh` and `devices-cli`, just as on an
   emulator. The artifacts hold nextest's JUnit report and the tests' output
   (`test-output.tar.gz`): the render suite's report, which shows its failed
   tests' images, and the images that other tests write. A test that fails gets
   one more try, which nextest reports as flaky when it passes.

Each test runs in a Java VM's process, as an app's code does, through the same
test host and runner that the emulator's runs use (`tools/android-test-host`).

## In CI

CI's `prepare-device-tests` job builds and uploads the `android-device-tests`
artifact when the `android` unit runs, without AWS credentials. The
`maplibre-device-farm` GitHub App runs the tests on AWS Device Farm without
holding a GitHub runner:

- When `prepare-device-tests` finishes (or when the `ci` workflow finishes, as a
  reconciliation fallback), the orchestrator reads `.config/device-farm.toml`
  and `tools/device-farm/android.yml` from `main`, downloads the artifact, stops
  any earlier in-flight run for the same branch, and schedules the run on the
  configured phones.
- While the phones run, it creates a `device-test / android` check run on the
  commit linking to the AWS console. When the plan skips `prepare-device-tests`,
  it reports the check run as passed.
- A poller checks active runs every minute, uploads each phone's logs and
  artifacts to the public results bucket, and settles the
  `device-test / android` check run with a per-phone JUnit summary and links to
  the published results. GitHub's Re-run button on the check run reschedules the
  run from the uploaded artifact.

Pull requests from forks get a run safely because the orchestrator always reads
`.config/device-farm.toml` and `android.yml` from `main` and never unpacks or
executes the package outside AWS Device Farm.

[maplibre/infra](https://github.com/maplibre/infra) manages the AWS side: the
`device-farm/` Lambda orchestrator and the `opentofu/device-farm` stack (the
`mln-rs` Device Farm project, Lambdas, queue, table, and results distribution).

## From your machine

With AWS credentials for MapLibre's account:

```sh
mise -E target-android run device-farm:package
mise run device-farm:run
```

The results land in `target/device-farm/results/<phone>/`. The phones are the
account's flat-rate devices, which maplibre-native's CI shares.
