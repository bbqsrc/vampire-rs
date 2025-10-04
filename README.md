# 🧛 Vampire

A minimalist Android test framework for Rust that enables running Rust tests on Android devices through dynamic loading.

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](https://github.com/vampire-rs/vampire)

## Why Vampire?

Named after the [vampire crab](https://en.wikipedia.org/wiki/Geosesarma_dennerle) (*Geosesarma dennerle*), a small purple crustacean native to Java. The name is a double pun: the crab lives in **Java** (the island), and this framework runs tests in **Java** (the Android platform). Like its namesake, Vampire dynamically loads into a host environment to do its work.

## Quick Start

1. **Install dependencies:**
   ```bash
   # Ensure you have Android SDK and NDK
   export ANDROID_SDK_ROOT=/path/to/android/sdk
   # Or on macOS, SDK is auto-detected from ~/Library/Android/sdk

   # Install cargo-ndk
   cargo install cargo-ndk
   ```

2. **Run tests:**
   ```bash
   # Run tests (builds + deploys + runs automatically)
   cargo run --bin vampire -- test

   # Show detailed test output including stdout/stderr
   cargo run --bin vampire -- test --nocapture

   # Force rebuild of APK even if already installed
   cargo run --bin vampire -- test --force
   ```

   The `vampire` CLI automatically:
   - ✅ Builds test library for Android (arm64-v8a)
   - ✅ Compiles and builds host APK (only when needed)
   - ✅ Installs APK to device (only if not already installed)
   - ✅ Deploys native library to app's private storage
   - ✅ Runs tests and displays results

## Configuration

### Android Permissions

Specify Android permissions your tests need in `Cargo.toml`:

```toml
[package.metadata.vampire]
permissions = [
    "android.permission.INTERNET",
    "android.permission.ACCESS_NETWORK_STATE",
    "android.permission.WRITE_EXTERNAL_STORAGE"
]
```

These permissions are automatically added to the generated AndroidManifest.xml when building the host APK.

### Maven Dependencies

If your tests require Android libraries (JARs/AARs), specify them using Maven coordinates in a Cargo-style format:

```toml
[package.metadata.vampire.dependencies]
"org.chromium.net:cronet-api" = "141.7340.3"
"androidx.annotation:annotation" = "1.7.0"
```

You can also use the object form for future extensibility:

```toml
[package.metadata.vampire.dependencies]
"org.chromium.net:cronet-api" = { version = "141.7340.3" }
```

Vampire will automatically:
- Download the artifacts and their transitive dependencies from Maven Central and Google Maven
- Resolve version conflicts using Maven's nearest-wins strategy
- Extract classes from AAR files
- Include them in the DEX compilation for the host APK
- Cache downloads in `target/vampire/maven-cache/` for faster builds

This allows your Rust tests to interact with Java classes via JNI.

## Build Script Setup

If you have Java sources that need to be compiled for Android (e.g., JNI callback classes), create a `build.rs` file in your package root:

```rust
fn main() {
    vampire_build::configure();
}
```

Add `vampire-build` to your build dependencies in `Cargo.toml`:

```toml
[build-dependencies]
vampire-build = { path = "../vampire/vampire-build" }  # or version from crates.io
```

### Java Source Directory

By default, `vampire-build` looks for Java sources in the `java/` directory of your package. Place your `.java` files there:

```
my-package/
├── Cargo.toml
├── build.rs
├── java/
│   └── com/
│       └── example/
│           └── MyCallback.java
└── src/
    └── lib.rs
```

### Custom Configuration

For advanced use cases, you can customize the builder:

```rust
fn main() {
    vampire_build::Builder::new()
        .java_dir("src/main/java")  // Custom Java source directory
        .target_sdk(33)              // Target SDK version (default: 30)
        .java_source("path/to/specific/File.java")  // Add specific file
        .configure();
}
```

### What It Does

`vampire-build` automatically:
- Enables `cfg(vampire)` for conditional compilation
- Detects Android target builds
- Finds and compiles all `.java` files in your Java directory
- Uses Java 8 compatibility for Android
- Converts `.class` files to DEX format (required for Android)
- Outputs `classes.dex` to `OUT_DIR`
- Sets up proper `cargo:rerun-if-changed` triggers

**Note:** DEX generation requires the Android SDK with build-tools installed. Set `ANDROID_SDK_ROOT` or `ANDROID_HOME` environment variable pointing to your SDK location.

## Writing Tests

### Basic Tests

```rust
use vampire;

// IMPORTANT: Re-export JNI_OnLoad so Java can initialize the runtime
pub use vampire::JNI_OnLoad;

#[vampire::test]
fn sync_test() {
    assert_eq!(2 + 2, 4);
}

#[vampire::test]
async fn async_test() {
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Your async test code
}
```

### Expected Failures

Use `should_panic` for tests that are expected to fail:

```rust
#[vampire::test(should_panic)]
fn test_expected_failure() {
    panic!("This failure is expected and will count as a pass");
}
```

### Android System Interaction

Access Android system properties and device information:

```rust
use vampire::android;

#[vampire::test]
fn test_android_system() {
    // Get Android version
    let version = android::get_android_version()
        .expect("Should get Android version");

    // Access app's private directory
    let files_dir = android::get_files_dir()
        .expect("Should get files directory");

    // Query device information
    let model = android::get_device_model();
    let is_emulator = android::is_emulator();

    // Check system properties
    let prop = android::get_system_property("ro.build.version.sdk");
}
```

## Cross-Platform Tests

The `#[vampire::test]` macro works on both Android and native platforms:

```rust
#[vampire::test]
fn test_works_everywhere() {
    assert_eq!(2 + 2, 4);
}
```

- **On Android**: Runs through the instrumentation framework
- **On native**: Expands to `#[test]` (or `#[tokio::test]` for async)

Run native tests with `cargo test`, Android tests with `cargo run --bin vampire -- test`.

### Tests in `tests/` Directory

Since Android requires all tests in a single cdylib, you need to explicitly include `tests/` files in your `lib.rs`:

```rust
// In src/lib.rs
#[cfg(vampire)]
#[path = "../tests"]
mod integration_tests {
    #[path = "my_test.rs"]
    mod my_test;

    #[path = "another_test.rs"]
    mod another_test;
}
```

The `vampire` CLI automatically sets `--cfg vampire` when building, so these modules are only included when building for Android tests.

## Architecture

- **Host APK**: Minimal instrumentation container that loads test libraries
- **Test Library**: Your Rust tests compiled to `.so` with automatic JNI registration
- **vampire CLI**: Coordinates building, deploying, and running tests

## Components

- `vampire-cli`: Main CLI tool (`cargo run --bin vampire`)
- `vampire-macro`: Proc macro providing `#[vampire::test]`
- `vampire`: Runtime library with JNI utilities and Android system access
- `vampire-example`: Example test suite demonstrating features