# iOS test host

Runs the tests built for iOS devices in an app's process, as an app's code runs,
so that they run on a phone: iOS runs no executable but an app's, and gives the
GPU only to an app in the foreground. AWS Device Farm's runs use it
(`tools/device-farm`).

The tests are built as dylibs, `-C link-arg=-dynamiclib`, which the app holds in
its Frameworks with a copy of the workspace
(`mise run device-farm:package ios`). Each launch of the app runs one of them:
`main.m` loads the dylib that its first argument names once the app is in the
foreground, and calls its `main` with the rest of its arguments, which it makes
the process's, where the Rust standard library reads them on Apple platforms.
The test harness runs all of the binary's tests in the app's process, as
`cargo test` runs them, and the app exits with its status. A relative
`CARGO_MANIFEST_DIR` names the test's package in the copy of the workspace,
where the test runs, and `MLN_TEST_OUTPUT` is in the app's data container.

- `build.sh` builds the app's executable and `Info.plist` into an app bundle,
  with Xcode.
- `devicectl device process launch --console` launches the app with the tests'
  arguments and environment, and shows the app's output.
