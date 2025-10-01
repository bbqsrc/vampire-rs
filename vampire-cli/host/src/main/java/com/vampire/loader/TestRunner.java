package com.vampire.loader;

import android.os.Bundle;
import android.util.Log;

public class TestRunner {
    private static final String TAG = "TestRunner";

    // Native method to get test manifest from Rust
    private static native TestMetadata[] getTestManifest();

    public Bundle runTests(String libPath) {
        Bundle results = new Bundle();

        try {
            // Load the native library (JNI_OnLoad will redirect stdout/stderr)
            Log.d(TAG, "Loading native library: " + libPath);
            System.load(libPath);

            // Get the test manifest from Rust
            TestMetadata[] manifest = getTestManifest();
            int totalTests = manifest.length;
            int passedTests = 0;

            Log.i(TAG, "Running " + totalTests + " tests");

            // Run each test
            for (TestMetadata test : manifest) {
                String testName = test.getName();
                boolean isAsync = test.isAsync();
                boolean shouldPanic = test.shouldPanic();

                String testType = (shouldPanic ? " (should_panic)" : "") + (isAsync ? " (async)" : "");
                Log.i(TAG, "Running test: " + testName + testType);

                try {
                    // The Rust macro already handles should_panic logic and returns
                    // the correct boolean: true=pass, false=fail (accounting for panics)
                    boolean passed = invokeTest(testName);
                    results.putBoolean(testName, passed);

                    if (passed) {
                        passedTests++;
                        Log.i(TAG, "Test " + testName + " PASSED");
                    } else {
                        Log.i(TAG, "Test " + testName + " FAILED");
                    }
                } catch (Exception e) {
                    Log.e(TAG, "Test " + testName + " threw exception", e);
                    results.putBoolean(testName, false);
                }
            }

            // Summary
            results.putInt("total_tests", totalTests);
            results.putInt("passed_tests", passedTests);
            results.putInt("failed_tests", totalTests - passedTests);

            Log.i(TAG, "Test run complete: " + passedTests + "/" + totalTests + " passed");

        } catch (Exception e) {
            Log.e(TAG, "Error running tests", e);
            results.putString("error", e.getMessage());
        }

        return results;
    }

    // Native method declarations are generated dynamically based on test names
    private native boolean invokeTestNative(String testName);

    private boolean invokeTest(String testName) {
        try {
            return invokeTestNative(testName);
        } catch (Exception e) {
            Log.e(TAG, "Failed to invoke test: " + testName, e);
            return false;
        }
    }
}
