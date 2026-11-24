// The test host: an iOS app that runs a test binary, built as a dylib, in its
// process, so that the tests run as an app's code does, in the foreground,
// where Metal takes the GPU work of an app (README.md).
//
// Its arguments are the binary's file name in the app's Frameworks directory,
// then the test harness's arguments. A relative CARGO_MANIFEST_DIR names the
// test's package in the app's copy of the workspace, where the test runs.
// MLN_TEST_OUTPUT is Documents/test-output in the app's data container. The
// app exits with the harness's status.

#import <UIKit/UIKit.h>
#include <crt_externs.h>
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

// Loads the binary and calls its main. Rust's standard library reads the
// arguments of a process on Apple platforms from _NSGetArgv rather than from
// main's, so the harness's arguments become the process's.
static int run_test(int argc, char** argv) {
  if (argc < 2) {
    fprintf(stderr, "test-host: no test binary given\n");
    return 127;
  }
  NSBundle* bundle = NSBundle.mainBundle;
  const char* package = getenv("CARGO_MANIFEST_DIR");
  if (package && package[0] != '/') {
    NSString* workspace =
      [bundle.bundlePath stringByAppendingPathComponent:@"workspace"];
    NSString* directory = [workspace stringByAppendingPathComponent:@(package)];
    setenv("CARGO_MANIFEST_DIR", directory.fileSystemRepresentation, 1);
    if (chdir(directory.fileSystemRepresentation) != 0) {
      perror("test-host: chdir");
      return 127;
    }
  }
  NSString* documents = NSSearchPathForDirectoriesInDomains(
                          NSDocumentDirectory, NSUserDomainMask, YES
  )
                          .firstObject;
  NSString* output = [documents stringByAppendingPathComponent:@"test-output"];
  setenv("MLN_TEST_OUTPUT", output.fileSystemRepresentation, 1);

  NSString* path =
    [bundle.privateFrameworksPath stringByAppendingPathComponent:@(argv[1])];
  void* binary = dlopen(path.fileSystemRepresentation, RTLD_NOW);
  if (!binary) {
    fprintf(stderr, "test-host: %s\n", dlerror());
    return 127;
  }
  int (*test_main)(int, char**) = (int (*)(int, char**))dlsym(binary, "main");
  if (!test_main) {
    fprintf(stderr, "test-host: %s\n", dlerror());
    return 127;
  }
  *_NSGetArgc() = argc - 1;
  *_NSGetArgv() = argv + 1;
  return test_main(argc - 1, argv + 1);
}

@interface SceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic) UIWindow* window;
@end

@implementation SceneDelegate
- (void)scene:(UIScene*)scene
  willConnectToSession:(UISceneSession*)session
               options:(UISceneConnectionOptions*)options {
  self.window = [[UIWindow alloc] initWithWindowScene:(UIWindowScene*)scene];
  self.window.rootViewController = [UIViewController new];
  [self.window makeKeyAndVisible];
}

// Once the app is in the foreground. The harness runs off the main thread,
// which UIKit keeps, and the device stays awake while it runs.
- (void)sceneDidBecomeActive:(UIScene*)scene {
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    UIApplication.sharedApplication.idleTimerDisabled = YES;
    int argc = *_NSGetArgc();
    char** argv = *_NSGetArgv();
    [NSThread detachNewThreadWithBlock:^{
      int status = run_test(argc, argv);
      fflush(NULL);
      exit(status);
    }];
  });
}
@end

@interface AppDelegate : UIResponder <UIApplicationDelegate>
@end

@implementation AppDelegate
@end

int main(int argc, char* argv[]) {
  @autoreleasepool {
    return UIApplicationMain(
      argc, argv, nil, NSStringFromClass(AppDelegate.class)
    );
  }
}
