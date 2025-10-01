use clap::{Parser, Subcommand};
use std::path::Path;
use std::fs;

mod android_sdk;
mod host_templates;

// Vampire constants
const HOST_PACKAGE: &str = "com.vampire.host";
const APK_NAME: &str = "vampire-host";
const OUTPUT_DIR: &str = "target/vampire";
const TARGET_SDK: u32 = 30;
const INSTRUMENTATION_CLASS: &str = "VampireInstrumentation";
const NDK_TARGET: &str = "arm64-v8a";
const RUST_TARGET: &str = "aarch64-linux-android";

#[derive(Parser)]
#[command(name = "vampire")]
#[command(about = "CLI tool for Vampire Android test framework")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the test library and APK
    Build {
        /// Only build the test library, not the APK
        #[arg(long)]
        lib_only: bool,
    },
    /// Run tests on connected Android device
    Test {
        /// Device ID to run tests on
        #[arg(short, long)]
        device: Option<String>,
        /// Force rebuild of APK even if up-to-date
        #[arg(short, long)]
        force: bool,
        /// Show test output (stdout/stderr from tests)
        #[arg(long)]
        nocapture: bool,
    },
    /// Package APK with test artifacts
    Package,
    /// Clean build artifacts
    Clean,
}

fn get_library_name() -> Result<String, String> {
    let cargo_toml_path = "Cargo.toml";
    let content = fs::read_to_string(cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    let cargo_toml: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    // Try to get lib name from [lib] section first
    if let Some(lib_name) = cargo_toml.get("lib")
        .and_then(|lib| lib.get("name"))
        .and_then(|name| name.as_str()) {
        return Ok(lib_name.to_string());
    }

    // Fallback to package name with hyphens replaced by underscores
    if let Some(package_name) = cargo_toml.get("package")
        .and_then(|pkg| pkg.get("name"))
        .and_then(|name| name.as_str()) {
        return Ok(package_name.replace('-', "_"));
    }

    Err("Could not find library name in Cargo.toml".to_string())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build { lib_only } => build_project(lib_only).await,
        Commands::Test {
            device,
            force,
            nocapture,
        } => run_tests(device, force, nocapture).await,
        Commands::Package => package_apk().await,
        Commands::Clean => clean_project().await,
    }
}

async fn build_project(lib_only: bool) {
    println!("🔨 Building Vampire project...");

    // Build the test library (tests will register themselves via inventory)
    println!("📚 Building test library...");
    let build_result = tokio::process::Command::new("cargo")
        .args(&["build", "--release"])
        .status()
        .await;

    match build_result {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "❌ Failed to build test library (exit code: {:?})",
                status.code()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("❌ Failed to run cargo build: {}", e);
            std::process::exit(1);
        }
    }

    // Build for Android target
    println!("📱 Building for {}", NDK_TARGET);

    let android_build = tokio::process::Command::new("cargo")
        .env("RUSTFLAGS", "--cfg vampire")
        .args(&["ndk", "-t", NDK_TARGET, "build", "--release"])
        .output()
        .await;

    match android_build {
        Ok(output) => {
            if !output.status.success() {
                eprintln!("❌ Failed to build for arm64-v8a");
                eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
                std::process::exit(1);
            }
            // Print stdout to see build progress
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        Err(e) => {
            eprintln!("❌ Failed to run cargo-ndk: {}", e);
            eprintln!("💡 Make sure cargo-ndk is installed: cargo install cargo-ndk");
            std::process::exit(1);
        }
    }

    if lib_only {
        println!("✅ Library build complete!");
        return;
    }

    // Build APK and package everything
    package_apk().await;
}

async fn package_apk() {
    println!("📦 Packaging APK...");

    // Find Android SDK
    let sdk = match android_sdk::AndroidSdk::find() {
        Ok(sdk) => {
            println!("📱 Using Android SDK: {}", sdk.sdk_path.display());
            sdk
        }
        Err(e) => {
            eprintln!("❌ Failed to find Android SDK: {}", e);
            eprintln!("💡 Set ANDROID_SDK_ROOT environment variable");
            return;
        }
    };

    let vampire_output = Path::new(&OUTPUT_DIR);
    let api_level = TARGET_SDK;

    // Build host APK
    if let Err(e) = build_host_apk(&sdk, vampire_output, api_level).await {
        eprintln!("❌ Failed to build host APK: {}", e);
        return;
    }

    println!("✅ APK packaged successfully!");
}

async fn build_host_apk(
    sdk: &android_sdk::AndroidSdk,
    output_dir: &Path,
    api_level: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("🏗️  Building host APK...");

    let build_dir = output_dir.join("host-build");
    std::fs::create_dir_all(&build_dir)?;

    // Write embedded host template files
    host_templates::write_host_files(&build_dir)?;

    std::fs::create_dir_all(build_dir.join("gen"))?;
    std::fs::create_dir_all(build_dir.join("obj"))?;

    let manifest = build_dir.join("AndroidManifest.xml");
    let res_dir = build_dir.join("res");
    let java_dir = build_dir.join("java");

    // Step 1: Generate R.java
    sdk.generate_r_java(&manifest, &res_dir, &build_dir.join("gen"), api_level)
        .await?;

    // Step 2: Compile Java sources
    let r_java = build_dir
        .join("gen")
        .join("com/vampire/host")
        .join("R.java");
    let instrumentation_java = java_dir
        .join("com/vampire/host")
        .join("VampireInstrumentation.java");
    let test_runner_java = java_dir.join("com/vampire/loader").join("TestRunner.java");
    let test_metadata_java = java_dir
        .join("com/vampire/loader")
        .join("TestMetadata.java");

    sdk.compile_java(
        &[
            &r_java,
            &instrumentation_java,
            &test_runner_java,
            &test_metadata_java,
        ],
        &sdk.sdk_path
            .join("platforms")
            .join(format!("android-{}", api_level))
            .join("android.jar"),
        &build_dir.join("obj"),
    )
    .await?;

    // Step 3: Convert to DEX
    let classes_dex = build_dir.join("classes.dex");
    sdk.convert_to_dex(&build_dir.join("obj"), &classes_dex)
        .await?;

    // Step 4: Package APK
    let unsigned_apk = build_dir.join(format!("{}-unsigned.apk", APK_NAME));
    sdk.package_apk(&manifest, &res_dir, &classes_dex, &unsigned_apk, api_level)
        .await?;

    // Step 5: Align APK (must be done before signing with apksigner)
    let aligned_apk = build_dir.join(format!("{}-aligned.apk", APK_NAME));
    sdk.align_apk(&unsigned_apk, &aligned_apk).await?;

    // Step 6: Sign APK with apksigner
    let keystore = pathos::user::home_dir()
        .map_err(|e| format!("Could not find home directory: {}", e))?
        .join(".android/debug.keystore");

    let signed_apk = build_dir.join(format!("{}.apk", APK_NAME));
    sdk.sign_apk(
        &aligned_apk,
        &signed_apk,
        &keystore,
        "android",
        "androiddebugkey",
    )
    .await?;

    // Copy to output directory
    let final_apk = output_dir.join(format!("{}.apk", APK_NAME));
    std::fs::copy(&signed_apk, &final_apk)?;

    println!("✅ Host APK created: {}", final_apk.display());
    Ok(())
}

async fn run_tests(device: Option<String>, force: bool, nocapture: bool) {
    println!("🧪 Running tests...");

    // Find Android SDK for adb
    let sdk = match android_sdk::AndroidSdk::find() {
        Ok(sdk) => sdk,
        Err(e) => {
            eprintln!("❌ Failed to find Android SDK: {}", e);
            return;
        }
    };

    // Prepare adb args (with device if specified)
    let device_args: Vec<String> = if let Some(device_id) = &device {
        vec!["-s".to_string(), device_id.clone()]
    } else {
        vec![]
    };

    // Step 1: Check if APK exists and needs to be updated
    let apk_path = format!("target/vampire/{}.apk", APK_NAME);
    let apk_exists = std::path::Path::new(&apk_path).exists();

    let needs_apk_build = if force {
        println!("🔨 Force rebuild requested");
        true
    } else if apk_exists {
        // Check if package is installed
        let mut list_args = device_args.clone();
        list_args.extend_from_slice(&[
            "shell".to_string(),
            "pm".to_string(),
            "list".to_string(),
            "packages".to_string(),
            HOST_PACKAGE.to_string(),
        ]);

        let installed = if let Ok(output) = sdk
            .run_adb(&list_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            .await
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.contains(&format!("package:{}", HOST_PACKAGE))
        } else {
            false
        };

        if installed {
            println!("✅ Host APK already installed, skipping rebuild");
            false
        } else {
            println!("📦 APK not installed on device");
            true
        }
    } else {
        println!("📦 APK not found, will build from scratch");
        true
    };

    // Step 2: Build everything (or just test library if APK is up-to-date)
    if needs_apk_build {
        // Build everything including APK
        build_project(false).await;

        // Install the newly built APK
        println!("📱 Installing host APK...");
        let mut install_args = device_args.clone();
        install_args.extend_from_slice(&["install".to_string(), "-r".to_string(), apk_path]);

        if let Err(e) = sdk
            .run_adb(&install_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            .await
        {
            eprintln!("❌ Failed to install APK: {}", e);
            return;
        }
    } else {
        // Just build test library, skip APK build
        println!("🔨 Building test library only...");
        build_project(true).await; // lib_only = true
    }

    // Step 2: Push native library to device
    println!("📚 Pushing native library...");

    let lib_name = match get_library_name() {
        Ok(name) => name,
        Err(e) => {
            eprintln!("❌ {}", e);
            return;
        }
    };

    let lib_path = format!("target/{}/release/lib{}.so", RUST_TARGET, lib_name);
    let lib_filename = format!("lib{}.so", lib_name);

    if !Path::new(&lib_path).exists() {
        eprintln!("❌ Native library not found: {}", lib_path);
        eprintln!("💡 Make sure you ran the build for Android targets");
        return;
    }

    let mut push_lib_args = device_args.clone();
    push_lib_args.extend_from_slice(&[
        "push".to_string(),
        lib_path.to_string(),
        "/data/local/tmp/".to_string(),
    ]);

    if let Err(e) = sdk
        .run_adb(&push_lib_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .await
    {
        eprintln!("❌ Failed to push native library: {}", e);
        return;
    }

    // Step 3: Clear app data and copy files to app's private directory
    println!("🧹 Clearing app data...");
    let mut clear_args = device_args.clone();
    clear_args.extend_from_slice(&[
        "shell".to_string(),
        "pm".to_string(),
        "clear".to_string(),
        HOST_PACKAGE.to_string(),
    ]);
    let _ = sdk
        .run_adb(&clear_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .await;

    println!("📋 Copying native library to app directory...");

    // Create files directory
    let mut mkdir_args = device_args.clone();
    mkdir_args.extend_from_slice(&[
        "shell".to_string(),
        "run-as".to_string(),
        HOST_PACKAGE.to_string(),
        "mkdir".to_string(),
        "-p".to_string(),
        "files".to_string(),
    ]);
    let _ = sdk
        .run_adb(&mkdir_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .await;

    // Copy native library
    let mut copy_lib_args = device_args.clone();
    copy_lib_args.extend_from_slice(&[
        "shell".to_string(),
        "run-as".to_string(),
        HOST_PACKAGE.to_string(),
        "cp".to_string(),
        format!("/data/local/tmp/{}", lib_filename),
        format!("files/{}", lib_filename),
    ]);

    if let Err(e) = sdk
        .run_adb(&copy_lib_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .await
    {
        eprintln!("❌ Failed to copy native library to app directory: {}", e);
        return;
    }

    // Remove from /data/local/tmp
    let mut rm_args = device_args.clone();
    rm_args.extend_from_slice(&[
        "shell".to_string(),
        "rm".to_string(),
        format!("/data/local/tmp/{}", lib_filename),
    ]);
    let _ = sdk
        .run_adb(&rm_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .await;

    // Step 5: Run instrumentation tests with paths in app directory
    println!("🧪 Running instrumentation tests...");

    // Clear logcat before running tests
    let mut clear_args = device_args.clone();
    clear_args.extend_from_slice(&["logcat".to_string(), "-c".to_string()]);
    let _ = sdk
        .run_adb(&clear_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .await;

    // Start logcat capture
    println!("\n--- Test Output ---");

    let adb_path = sdk.sdk_path.join("platform-tools").join("adb");
    let mut logcat_cmd = tokio::process::Command::new(adb_path);

    if let Some(device_id) = &device {
        logcat_cmd.args(&["-s", device_id]);
    }

    // With nocapture: show all TestRunner logs (I, D, E)
    // Without nocapture: show only Info level (test results)
    if nocapture {
        logcat_cmd.args(&["logcat", "-v", "threadtime", "-s", "TestRunner:*"]);
    } else {
        logcat_cmd.args(&["logcat", "-v", "threadtime", "-s", "TestRunner:I"]);
    }

    logcat_cmd.stdout(std::process::Stdio::piped());
    logcat_cmd.stderr(std::process::Stdio::null());

    let mut child = logcat_cmd.spawn().ok();

    // Spawn a task to read and filter logcat output
    let logcat_handle = if let Some(ref mut child) = child {
        if let Some(stdout) = child.stdout.take() {
            let nocapture_flag = nocapture;
            Some(tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    // Skip logcat system messages
                    if line.starts_with("--------- beginning of") {
                        continue;
                    }

                    // threadtime format: MM-DD HH:MM:SS.mmm  PID  TID LEVEL TAG: message
                    // Extract priority level and message
                    if let Some(message_start) = line.find(": ") {
                        let message = &line[message_start + 2..];

                        // Find priority level (I, D, E, W, etc.) - it's before the tag
                        let is_info = line.contains(" I ") || line.contains(" I/");
                        let is_error = line.contains(" E ") || line.contains(" E/");

                        if is_info {
                            // Info level: just print the message (test results)
                            println!("{}", message);
                        } else if nocapture_flag {
                            // Debug/Error level with --nocapture: add prefix
                            if is_error {
                                println!("\x1b[31merr:\x1b[0m {}", message);
                            } else {
                                println!("\x1b[90mout:\x1b[0m {}", message);
                            }
                        }
                    }
                }
            }))
        } else {
            None
        }
    } else {
        None
    };

    let app_files_dir = format!("/data/data/{}/files", HOST_PACKAGE);
    let mut test_args = device_args.clone();
    test_args.extend_from_slice(&[
        "shell".to_string(),
        "am".to_string(),
        "instrument".to_string(),
        "-w".to_string(),
        "-e".to_string(),
        "lib_path".to_string(),
        format!("{}/{}", app_files_dir, lib_filename),
        format!("{}/.{}", HOST_PACKAGE, INSTRUMENTATION_CLASS),
    ]);

    match sdk
        .run_adb(&test_args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .await
    {
        Ok(output) => {
            // Print the output
            let stdout = String::from_utf8_lossy(&output.stdout);
            // print!("{}", stdout);

            // Stop logcat capture
            if let Some(handle) = logcat_handle {
                handle.abort();
            }
            println!("--- End Output ---\n");

            // Parse results
            parse_test_results(&stdout);
        }
        Err(e) => {
            if let Some(handle) = logcat_handle {
                handle.abort();
            }
            eprintln!("❌ Tests failed: {}", e);
        }
    }
}

fn parse_test_results(output: &str) {
    let mut total = 0;
    let mut passed = 0;
    let mut failed = 0;

    for line in output.lines() {
        if line.contains("INSTRUMENTATION_RESULT: total_tests=") {
            total = line
                .split('=')
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        } else if line.contains("INSTRUMENTATION_RESULT: passed_tests=") {
            passed = line
                .split('=')
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        } else if line.contains("INSTRUMENTATION_RESULT: failed_tests=") {
            failed = line
                .split('=')
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        }
    }

    println!("\n📊 Test Results:");
    println!("  Total:  {}", total);
    println!("  ✅ Passed: {}", passed);
    println!("  ❌ Failed: {}", failed);

    if failed == 0 && total > 0 {
        println!("\n🎉 All tests passed!");
    } else if failed > 0 {
        println!("\n⚠️  Some tests failed");
    }
}

async fn clean_project() {
    println!("🧹 Cleaning project...");

    let clean_result = tokio::process::Command::new("cargo")
        .args(&["clean"])
        .status()
        .await;

    if clean_result.is_err() || !clean_result.unwrap().success() {
        eprintln!("❌ Failed to clean project");
        return;
    }

    // Clean vampire output directory

    if let Err(e) = std::fs::remove_dir_all(&OUTPUT_DIR) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("⚠️  Could not remove {}: {}", OUTPUT_DIR, e);
        }
    }

    println!("✅ Project cleaned!");
}
