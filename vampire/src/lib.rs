// Re-export JNI types for convenience
pub use jni::{
    objects::{JClass, JObject},
    sys::{jboolean, jstring, JavaVM, JNI_FALSE, JNI_TRUE},
    JNIEnv,
};

// Re-export inventory for macro use
pub use inventory;

// Re-export the test macro
pub use vampire_macro::test;

use std::ffi::CString;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

/// Metadata about a test function
#[derive(Debug, Clone)]
pub struct TestMetadata {
    pub name: &'static str,
    pub r#async: bool,
    pub should_panic: bool,
}

inventory::collect!(TestMetadata);

/// Metadata about a test function, including its function pointer
pub struct TestEntry {
    pub metadata: TestMetadata,
    pub test_fn: fn() -> bool,
}

inventory::collect!(TestEntry);

/// Get all registered tests as an array of TestMetadata objects
#[no_mangle]
pub extern "system" fn Java_com_vampire_loader_TestRunner_getTestManifest(
    mut env: JNIEnv,
    _class: JClass,
) -> jni::sys::jobjectArray {
    use jni::objects::{JObject, JValue};

    // Find the TestMetadata class
    let test_metadata_class = match env.find_class("com/vampire/loader/TestMetadata") {
        Ok(cls) => cls,
        Err(_) => return JObject::null().into_raw(),
    };

    // Collect all tests
    let tests: Vec<&TestMetadata> = inventory::iter::<TestEntry>()
        .map(|entry| &entry.metadata)
        .collect();

    // Create object array
    let array =
        match env.new_object_array(tests.len() as i32, &test_metadata_class, JObject::null()) {
            Ok(arr) => arr,
            Err(_) => return JObject::null().into_raw(),
        };

    // Fill the array with TestMetadata objects
    for (i, test) in tests.iter().enumerate() {
        // Create Java string for test name
        let name_jstring = match env.new_string(test.name) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Create TestMetadata object: new TestMetadata(String name, boolean isAsync, boolean shouldPanic)
        let test_obj = match env.new_object(
            &test_metadata_class,
            "(Ljava/lang/String;ZZ)V",
            &[
                JValue::Object(&name_jstring),
                JValue::Bool(test.r#async as u8),
                JValue::Bool(test.should_panic as u8),
            ],
        ) {
            Ok(obj) => obj,
            Err(_) => continue,
        };

        // Set array element
        if env
            .set_object_array_element(&array, i as i32, test_obj)
            .is_err()
        {
            continue;
        }
    }

    array.into_raw()
}

/// Invoke a test by name
#[no_mangle]
pub extern "system" fn Java_com_vampire_loader_TestRunner_invokeTestNative(
    mut env: JNIEnv,
    _class: JClass,
    test_name: jstring,
) -> jboolean {
    let test_name_obj = unsafe { JObject::from_raw(test_name) };
    let name: String = match env.get_string(&test_name_obj.into()) {
        Ok(s) => s.into(),
        Err(_) => return JNI_FALSE,
    };

    // Find the test by name
    for entry in inventory::iter::<TestEntry>() {
        if entry.metadata.name == name {
            let passed = (entry.test_fn)();
            return if passed { JNI_TRUE } else { JNI_FALSE };
        }
    }

    JNI_FALSE
}

// Dynamically load liblog.so since it's only available on device
use std::sync::LazyLock;

static LIBLOG: LazyLock<Result<libloading::Library, Arc<libloading::Error>>> =
    LazyLock::new(|| unsafe { libloading::Library::new("liblog.so").map_err(Arc::new) });

type AndroidLogWriteFn = unsafe extern "C" fn(
    prio: libc::c_int,
    tag: *const libc::c_char,
    text: *const libc::c_char,
) -> libc::c_int;

fn get_android_log_write() -> Option<libloading::Symbol<'static, AndroidLogWriteFn>> {
    unsafe {
        LIBLOG
            .as_ref()
            .ok()
            .and_then(|lib| lib.get(b"__android_log_write\0").ok())
    }
}

/// Log directly to Android logcat with a custom tag
pub fn android_log(priority: i32, tag: &str, message: &str) {
    unsafe {
        if let Some(log_write) = get_android_log_write() {
            if let (Ok(c_tag), Ok(c_msg)) = (CString::new(tag), CString::new(message)) {
                log_write(priority, c_tag.as_ptr(), c_msg.as_ptr());
            }
        }
    }
}

/// Log info message (priority 4)
pub fn log_info(tag: &str, message: &str) {
    android_log(4, tag, message);
}

/// Log debug message (priority 3)
pub fn log_debug(tag: &str, message: &str) {
    android_log(3, tag, message);
}

/// Log error message (priority 6)
pub fn log_error(tag: &str, message: &str) {
    android_log(6, tag, message);
}

// Global JavaVM pointer for async runtime access
static GLOBAL_VM: AtomicPtr<JavaVM> = AtomicPtr::new(null_mut());

#[no_mangle]
pub extern "system" fn JNI_OnLoad(
    vm: *mut JavaVM,
    _reserved: *mut std::ffi::c_void,
) -> jni::sys::jint {
    GLOBAL_VM.store(vm, Ordering::Release);

    redirect_stdout_to_logcat();

    jni::sys::JNI_VERSION_1_6
}

/// Get the JavaVM pointer stored during JNI_OnLoad
pub fn get_java_vm() -> Option<*mut JavaVM> {
    let vm_ptr = GLOBAL_VM.load(Ordering::Acquire);
    if vm_ptr.is_null() {
        None
    } else {
        Some(vm_ptr)
    }
}

/// Convenience function for JNI operations
pub fn with_jni_env<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut JNIEnv) -> R,
{
    let vm_ptr = get_java_vm()?;

    unsafe {
        let vm = jni::JavaVM::from_raw(vm_ptr).ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        Some(f(&mut env))
    }
}

/// Redirect stdout and stderr to Android logcat
/// This allows println! and eprintln! to show up in logcat
pub fn redirect_stdout_to_logcat() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::io::FromRawFd;

    unsafe {
        if get_android_log_write().is_none() {
            panic!("liblog.so not found, cannot redirect stdout/stderr");
        }

        // Create separate pipes for stdout and stderr
        let mut stdout_pfd: [i32; 2] = [0; 2];
        let mut stderr_pfd: [i32; 2] = [0; 2];

        if libc::pipe(stdout_pfd.as_mut_ptr()) == 0 {
            libc::dup2(stdout_pfd[1], libc::STDOUT_FILENO);
            libc::close(stdout_pfd[1]);

            let read_fd = stdout_pfd[0];
            std::thread::spawn(move || {
                let tag = CString::new("TestRunner").unwrap();
                let log_fn = get_android_log_write();

                let file = std::fs::File::from_raw_fd(read_fd);
                let reader = BufReader::new(file);

                for line in reader.lines().filter_map(Result::ok) {
                    if let Ok(c_line) = CString::new(line) {
                        if let Some(log_write) = &log_fn {
                            log_write(3, tag.as_ptr(), c_line.as_ptr());
                        }
                    }
                }
            });
        }

        if libc::pipe(stderr_pfd.as_mut_ptr()) == 0 {
            libc::dup2(stderr_pfd[1], libc::STDERR_FILENO);
            libc::close(stderr_pfd[1]);

            let read_fd = stderr_pfd[0];
            std::thread::spawn(move || {
                let tag = CString::new("TestRunner").unwrap();
                let log_fn = get_android_log_write();

                let file = std::fs::File::from_raw_fd(read_fd);
                let reader = BufReader::new(file);

                for line in reader.lines().filter_map(Result::ok) {
                    if let Ok(c_line) = CString::new(line) {
                        if let Some(log_write) = &log_fn {
                            log_write(6, tag.as_ptr(), c_line.as_ptr());
                        }
                    }
                }
            });
        }
    }
}

/// Android system helpers
pub mod android {
    use super::with_jni_env;
    use jni::objects::JString;

    /// Get Android system property (e.g., "ro.build.version.release")
    pub fn get_system_property(key: &str) -> Option<String> {
        with_jni_env(|env| {
            // Use android.os.SystemProperties for Android-specific properties
            let system_properties_class = env.find_class("android/os/SystemProperties").ok()?;

            let key_jstring = env.new_string(key).ok()?;
            let default_value = env.new_string("").ok()?;

            let result = env
                .call_static_method(
                    system_properties_class,
                    "get",
                    "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                    &[(&key_jstring).into(), (&default_value).into()],
                )
                .ok()?;

            let result_string: JString = result.l().ok()?.into();
            let result_str: String = env.get_string(&result_string).ok()?.into();

            if result_str.is_empty() {
                None
            } else {
                Some(result_str)
            }
        })?
    }

    /// Get app files directory path
    pub fn get_files_dir() -> Option<String> {
        with_jni_env(|env| {
            // Get the application context through ActivityThread
            let activity_thread_class = env.find_class("android/app/ActivityThread").ok()?;

            let app = env
                .call_static_method(
                    activity_thread_class,
                    "currentApplication",
                    "()Landroid/app/Application;",
                    &[],
                )
                .ok()?;

            let context = app.l().ok()?;

            let files_dir = env
                .call_method(&context, "getFilesDir", "()Ljava/io/File;", &[])
                .ok()?;
            let file_obj = files_dir.l().ok()?;

            let path_result = env
                .call_method(&file_obj, "getAbsolutePath", "()Ljava/lang/String;", &[])
                .ok()?;
            let path_string: JString = path_result.l().ok()?.into();
            let path_str: String = env.get_string(&path_string).ok()?.into();
            Some(path_str)
        })?
    }

    /// Get device CPU architecture
    pub fn get_cpu_architecture() -> Option<String> {
        // Try Build.SUPPORTED_ABIS first (API 21+)
        use jni::objects::JObjectArray;

        with_jni_env(|env| {
            let build_class = env.find_class("android/os/Build").ok()?;

            // Get SUPPORTED_ABIS field (String array)
            let abis = env
                .get_static_field(build_class, "SUPPORTED_ABIS", "[Ljava/lang/String;")
                .ok()?;
            let abis_obj = abis.l().ok()?;
            let abis_array = JObjectArray::from(abis_obj);

            // Get the first ABI (primary architecture)
            let first_abi = env.get_object_array_element(&abis_array, 0).ok()?;
            let abi_string: JString = first_abi.into();
            let abi_str: String = env.get_string(&abi_string).ok()?.into();
            Some(abi_str)
        })?
    }

    /// Get Android version
    pub fn get_android_version() -> Option<String> {
        get_system_property("ro.build.version.release")
    }

    /// Get device model
    pub fn get_device_model() -> Option<String> {
        get_system_property("ro.product.model")
    }

    /// Check if running on emulator
    pub fn is_emulator() -> bool {
        let fingerprint = get_system_property("ro.build.fingerprint").unwrap_or_default();
        let model = get_device_model().unwrap_or_default();

        fingerprint.contains("generic")
            || fingerprint.contains("unknown")
            || model.contains("Emulator")
            || model.contains("Android SDK")
    }

    /// Get available memory in bytes
    pub fn get_available_memory() -> Option<i64> {
        with_jni_env(|env| {
            let runtime_class = env.find_class("java/lang/Runtime").ok()?;

            let runtime = env
                .call_static_method(runtime_class, "getRuntime", "()Ljava/lang/Runtime;", &[])
                .ok()?;
            let runtime_obj = runtime.l().ok()?;

            let result = env
                .call_method(&runtime_obj, "freeMemory", "()J", &[])
                .ok()?;
            Some(result.j().ok()?)
        })?
    }
}
