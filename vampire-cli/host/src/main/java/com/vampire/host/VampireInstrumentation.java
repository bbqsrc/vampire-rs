package com.vampire.host;

import android.app.Activity;
import android.app.Instrumentation;
import android.os.Bundle;
import android.util.Log;
import com.vampire.loader.TestRunner;

public class VampireInstrumentation extends Instrumentation {
    private static final String TAG = "VampireInstrumentation";

    @Override
    public void onCreate(Bundle arguments) {
        super.onCreate(arguments);

        Log.d(TAG, "Vampire instrumentation starting");

        try {
            // Get library path from instrumentation arguments
            String libPath = arguments.getString("lib_path");

            if (libPath == null) {
                throw new IllegalArgumentException("Missing lib_path argument");
            }

            Log.d(TAG, "Library path: " + libPath);

            // Create TestRunner and run tests
            TestRunner testRunner = new TestRunner();
            Bundle results = testRunner.runTests(libPath);

            Log.d(TAG, "Tests completed successfully");
            finish(Activity.RESULT_OK, results);

        } catch (Exception e) {
            Log.e(TAG, "Error running tests", e);
            Bundle results = new Bundle();
            results.putString("error", e.getMessage());
            finish(Activity.RESULT_CANCELED, results);
        }
    }
}