use std::fs;
use std::time::Duration;
use vampire::android;

pub use vampire::JNI_OnLoad;

// Include integration tests from tests/ directory when building for Android
// This is necessary because tests/ creates separate binaries on native,
// but we need everything in one cdylib for Android
#[cfg(vampire)]
#[path = "../tests"]
mod integration_tests {
    #[path = "integration_test.rs"]
    mod integration_test;
}

#[vampire::test]
fn another() {
    println!("H");
    println!("  H");
    println!(" HHH");
    println!("H   H");
    println!("     HHHHH");
}

#[vampire::test]
fn test_android_version_detection() {
    // Test we can read actual Android system properties
    let version = android::get_android_version().expect("Should be able to get Android version");

    println!("Running on Android {}", version);

    // Android versions are numeric (like "11", "12", "13")
    let version_num: f32 = version
        .parse()
        .expect("Android version should be parseable as number");

    // We target API 24+ (Android 7.0+)
    assert!(
        version_num >= 7.0,
        "Running on unsupported Android version: {}",
        version
    );
}

#[vampire::test]
fn test_device_architecture() {
    // Test CPU architecture detection
    let arch = android::get_cpu_architecture().expect("Should be able to get CPU architecture");

    println!("CPU Architecture: {}", arch);

    // Should be one of the supported architectures
    assert!(
        arch.contains("aarch64") || arch.contains("arm") || arch.contains("x86"),
        "Unexpected architecture: {}",
        arch
    );
}

#[vampire::test]
fn test_app_files_directory_access() {
    // Test we can access app's private directory
    let files_dir = android::get_files_dir().expect("Should be able to get app files directory");

    println!("App files directory: {}", files_dir);

    // Create a test file in our app directory
    let test_file_path = format!("{}/vampire_test.txt", files_dir);
    let test_content = "Vampire test data";

    fs::write(&test_file_path, test_content)
        .expect("Should be able to write to app files directory");

    let read_content = fs::read_to_string(&test_file_path)
        .expect("Should be able to read from app files directory");

    assert_eq!(read_content, test_content);

    // Clean up
    fs::remove_file(&test_file_path).ok();
}

#[vampire::test]
fn test_memory_information() {
    // Test we can query system memory info
    let available_memory =
        android::get_available_memory().expect("Should be able to get memory information");

    println!(
        "Available memory: {} bytes ({:.2} MB)",
        available_memory,
        available_memory as f64 / 1024.0 / 1024.0
    );

    // Should have at least 10MB available (very conservative)
    assert!(
        available_memory > 10 * 1024 * 1024,
        "Device should have at least 10MB memory available"
    );
}

#[vampire::test]
fn test_device_properties() {
    // Test device model detection
    let model = android::get_device_model().expect("Should be able to get device model");

    println!("Device model: {}", model);
    assert!(!model.is_empty(), "Device model should not be empty");

    // Test emulator detection
    let is_emulator = android::is_emulator();
    println!("Running on emulator: {}", is_emulator);

    // Log device type for debugging
    if is_emulator {
        println!("Detected emulator environment");
    } else {
        println!("Detected real device");
    }
}

#[vampire::test]
async fn test_filesystem_constraints() {
    // Test that we can't write to system directories (Android sandboxing)
    let root_write = fs::write("/vampire_test.txt", "should fail");
    assert!(
        root_write.is_err(),
        "Should not be able to write to root directory"
    );

    let system_write = fs::write("/system/vampire_test.txt", "should fail");
    assert!(
        system_write.is_err(),
        "Should not be able to write to /system"
    );

    // Test we can write to our app directory
    let files_dir = android::get_files_dir().expect("Should get app files directory");

    let app_file = format!("{}/test_constraints.txt", files_dir);
    let app_write = fs::write(&app_file, "should succeed");
    assert!(
        app_write.is_ok(),
        "Should be able to write to app directory"
    );

    // Clean up
    fs::remove_file(&app_file).ok();
}

#[vampire::test]
fn test_system_property_access() {
    // Test various system properties that should exist on all Android devices
    let properties = [
        "ro.build.version.release", // Android version
        "ro.product.model",         // Device model
        "ro.build.fingerprint",     // Build fingerprint
        "ro.product.manufacturer",  // Manufacturer
    ];

    for prop in &properties {
        if let Some(value) = android::get_system_property(prop) {
            println!("{}: {}", prop, value);
            assert!(!value.is_empty(), "Property {} should not be empty", prop);
        } else {
            panic!("Should be able to read system property: {}", prop);
        }
    }
}

#[vampire::test(should_panic)]
fn test_that_should_fail() {
    // This test intentionally fails to verify error handling
    panic!("This test is supposed to fail for error handling verification");
}
