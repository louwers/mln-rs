// The native half of TestHost.java: loads a test executable into the Java
// VM's process and calls its main. The package builds the tests with main
// exported (mise-tasks/device-farm/package), and the executable reaches the
// VM through JNI_GetCreatedJavaVMs.

#include <dlfcn.h>
#include <jni.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

JNIEXPORT jint JNICALL Java_org_maplibre_mlnrs_TestHost_run(
  JNIEnv* env, jclass host, jobjectArray arguments
) {
  (void)host;
  jsize count = (*env)->GetArrayLength(env, arguments);
  char** argv = calloc((size_t)count + 1, sizeof(char*));
  if (!argv) {
    return 127;
  }
  for (jsize i = 0; i < count; i++) {
    jstring argument = (*env)->GetObjectArrayElement(env, arguments, i);
    const char* chars = (*env)->GetStringUTFChars(env, argument, NULL);
    argv[i] = strdup(chars);
    (*env)->ReleaseStringUTFChars(env, argument, chars);
    (*env)->DeleteLocalRef(env, argument);
  }
  void* executable = dlopen(argv[0], RTLD_NOW);
  if (!executable) {
    fprintf(stderr, "test-host: %s\n", dlerror());
    return 127;
  }
  int (*test_main)(int, char**) =
    (int (*)(int, char**))dlsym(executable, "main");
  if (!test_main) {
    fprintf(stderr, "test-host: %s\n", dlerror());
    return 127;
  }
  return test_main((int)count, argv);
}
