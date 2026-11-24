package org.maplibre.mlnrs;

import java.util.Arrays;

/**
 * Runs a test executable built for Android inside a Java VM's process, so
 * that its tests have the Java VM that an app's code has: launcher.sh starts
 * this class with app_process, which starts Android's runtime as it does for
 * an app, and the tests' HTTP and locale services find the VM.
 *
 * <p>Arguments: the test host library (test-host.c), then the test executable
 * and its arguments. Exits with the executable's exit status.
 */
public final class TestHost {
    private TestHost() {}

    public static void main(String[] arguments) {
        System.load(arguments[0]);
        System.exit(run(Arrays.copyOfRange(arguments, 1, arguments.length)));
    }

    /** Runs the executable that {@code arguments[0]} names, with these arguments. */
    private static native int run(String[] arguments);
}
