# 🧛 Vampire

A minimalist Android test framework for Rust that enables running Rust tests on Android devices through dynamic loading.

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](https://github.com/vampire-rs/vampire)

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

## Writing Tests

### Basic Tests

```rust
use vampire;

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