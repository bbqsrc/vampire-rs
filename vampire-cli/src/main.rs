use clap::{Parser, Subcommand};
use xmlem::NewElement;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

mod android_sdk;
mod host_templates;
mod maven;

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
        /// Show full logcat trace (all output until killed)
        #[arg(long)]
        trace: bool,
        /// Additional logcat filters to include (can be specified multiple times)
        /// Format: TAG:LEVEL (e.g. "chromium:D" or "MyTag:V")
        #[arg(long = "logcat-filter", action = clap::ArgAction::Append)]
        logcat_filters: Vec<String>,
        /// Run only tests whose names contain this substring
        #[arg(long)]
        test: Option<String>,
    },
    /// Package APK with test artifacts
    Package,
    /// Clean build artifacts
    Clean,
    /// Show resolved Maven dependencies (dry-run)
    Deps,
    /// Update Maven dependencies and regenerate lock file
    Update,
}

fn get_library_name() -> Result<String, String> {
    let cargo_toml_path = "Cargo.toml";
    let content = fs::read_to_string(cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    let cargo_toml: toml::Value =
        toml::from_str(&content).map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    // Try to get lib name from [lib] section first
    if let Some(lib_name) = cargo_toml
        .get("lib")
        .and_then(|lib| lib.get("name"))
        .and_then(|name| name.as_str())
    {
        return Ok(lib_name.to_string());
    }

    // Fallback to package name with hyphens replaced by underscores
    if let Some(package_name) = cargo_toml
        .get("package")
        .and_then(|pkg| pkg.get("name"))
        .and_then(|name| name.as_str())
    {
        return Ok(package_name.replace('-', "_"));
    }

    Err("Could not find library name in Cargo.toml".to_string())
}

fn get_android_permissions() -> Vec<String> {
    let cargo_toml_path = "Cargo.toml";
    let content = match fs::read_to_string(cargo_toml_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let cargo_toml: toml::Value = match toml::from_str(&content) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    cargo_toml
        .get("package")
        .and_then(|pkg| pkg.get("metadata"))
        .and_then(|meta| meta.get("vampire"))
        .and_then(|vampire| vampire.get("permissions"))
        .and_then(|perms| perms.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn get_maven_dependencies() -> Vec<String> {
    let cargo_toml_path = "Cargo.toml";
    let content = match fs::read_to_string(cargo_toml_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let cargo_toml: toml::Value = match toml::from_str(&content) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let mut coordinates = Vec::new();

    if let Some(deps) = cargo_toml
        .get("package")
        .and_then(|pkg| pkg.get("metadata"))
        .and_then(|meta| meta.get("vampire"))
        .and_then(|vampire| vampire.get("dependencies"))
        .and_then(|deps| deps.as_table())
    {
        for (coord, value) in deps {
            // Support both string version and object with version field
            let version = if let Some(version_str) = value.as_str() {
                version_str.to_string()
            } else if let Some(version_str) = value.get("version").and_then(|v| v.as_str()) {
                version_str.to_string()
            } else {
                continue;
            };

            coordinates.push(format!("{}:{}", coord, version));
        }
    }

    coordinates
}

fn get_android_resources() -> HashMap<String, toml::Table> {
    let cargo_toml_path = "Cargo.toml";
    let content = match fs::read_to_string(cargo_toml_path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };

    let cargo_toml: toml::Value = match toml::from_str(&content) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };

    cargo_toml
        .get("package")
        .and_then(|pkg| pkg.get("metadata"))
        .and_then(|meta| meta.get("vampire"))
        .and_then(|vampire| vampire.get("res"))
        .and_then(|res| res.get("values"))
        .and_then(|values| values.as_table())
        .map(|table| {
            table.iter()
                .filter_map(|(filename, value)| {
                    value.as_table().map(|t| (filename.clone(), t.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct ManifestComponent {
    pub name: String,
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Default)]
pub struct ManifestComponents {
    pub services: Vec<ManifestComponent>,
    pub receivers: Vec<ManifestComponent>,
}

fn get_manifest_components() -> ManifestComponents {
    let cargo_toml_path = "Cargo.toml";
    let content = match fs::read_to_string(cargo_toml_path) {
        Ok(c) => c,
        Err(_) => return ManifestComponents::default(),
    };

    let cargo_toml: toml::Value = match toml::from_str(&content) {
        Ok(t) => t,
        Err(_) => return ManifestComponents::default(),
    };

    let manifest_section = cargo_toml
        .get("package")
        .and_then(|pkg| pkg.get("metadata"))
        .and_then(|meta| meta.get("vampire"))
        .and_then(|vampire| vampire.get("manifest"));

    let mut components = ManifestComponents::default();

    if let Some(manifest) = manifest_section {
        // Parse services
        if let Some(services) = manifest.get("service").and_then(|s| s.as_array()) {
            for service in services {
                if let Some(table) = service.as_table() {
                    if let Some(name) = table.get("name").and_then(|n| n.as_str()) {
                        let mut attrs = HashMap::new();
                        for (key, value) in table {
                            if key != "name" {
                                if let Some(s) = value.as_str() {
                                    attrs.insert(key.clone(), s.to_string());
                                } else if let Some(b) = value.as_bool() {
                                    attrs.insert(key.clone(), b.to_string());
                                }
                            }
                        }
                        components.services.push(ManifestComponent {
                            name: name.to_string(),
                            attributes: attrs,
                        });
                    }
                }
            }
        }

        // Parse receivers
        if let Some(receivers) = manifest.get("receiver").and_then(|r| r.as_array()) {
            for receiver in receivers {
                if let Some(table) = receiver.as_table() {
                    if let Some(name) = table.get("name").and_then(|n| n.as_str()) {
                        let mut attrs = HashMap::new();
                        for (key, value) in table {
                            if key != "name" {
                                if let Some(s) = value.as_str() {
                                    attrs.insert(key.clone(), s.to_string());
                                } else if let Some(b) = value.as_bool() {
                                    attrs.insert(key.clone(), b.to_string());
                                }
                            }
                        }
                        components.receivers.push(ManifestComponent {
                            name: name.to_string(),
                            attributes: attrs,
                        });
                    }
                }
            }
        }
    }

    components
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
            trace,
            logcat_filters,
            test,
        } => run_tests(device, force, nocapture, trace, logcat_filters, test).await,
        Commands::Package => {
            if let Err(e) = package_apk().await {
                eprintln!("❌ Failed to package APK: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Clean => clean_project().await,
        Commands::Deps => show_dependencies().await,
        Commands::Update => update_dependencies().await,
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

    let rustflags = format!("--cfg vampire");

    let android_build = tokio::process::Command::new("cargo")
        .env("RUSTFLAGS", &rustflags)
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
    if let Err(e) = package_apk().await {
        eprintln!("❌ Failed to package APK: {}", e);
        std::process::exit(1);
    }
}

async fn package_apk() -> Result<(), Box<dyn std::error::Error>> {
    println!("📦 Packaging APK...");

    // Find Android SDK
    let sdk = android_sdk::AndroidSdk::find().map_err(|e| {
        eprintln!("❌ Failed to find Android SDK: {}", e);
        eprintln!("💡 Set ANDROID_SDK_ROOT environment variable");
        e
    })?;

    println!("📱 Using Android SDK: {}", sdk.sdk_path.display());

    let vampire_output = Path::new(&OUTPUT_DIR);
    let api_level = TARGET_SDK;

    // Build host APK
    build_host_apk(&sdk, vampire_output, api_level).await?;

    println!("✅ APK packaged successfully!");
    Ok(())
}

fn merge_manifests(
    host_manifest: &Path,
    aar_manifests: &[PathBuf],
    output_manifest: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let host_content = std::fs::read_to_string(host_manifest)?;
    let mut host_doc = xmlem::Document::from_str(&host_content)?;

    let host_package = host_doc.root()
        .attribute(&host_doc, "package")
        .ok_or("Host manifest missing package attribute")?
        .to_string();

    // Ensure xmlns:tools namespace is present
    if host_doc.root().attribute(&host_doc, "xmlns:tools").is_none() {
        host_doc.root().set_attribute(&mut host_doc, "xmlns:tools", "http://schemas.android.com/tools");
    }

    let mut app_element = None;
    let manifest_element = host_doc.root();

    for child in manifest_element.children(&host_doc) {
        if child.name(&host_doc) == "application" {
            app_element = Some(child);
            break;
        }
    }

    let app_elem = app_element.ok_or("Host manifest missing <application> element")?;

    for aar_manifest_path in aar_manifests {
        let aar_content = std::fs::read_to_string(aar_manifest_path)?;
        let aar_doc = xmlem::Document::from_str(&aar_content)?;

        for child in aar_doc.root().children(&aar_doc) {
            if child.name(&aar_doc) == "application" {
                for aar_app_child in child.children(&aar_doc) {
                    let elem_name = aar_app_child.name(&aar_doc);
                    if matches!(elem_name, "service" | "receiver" | "provider" | "activity" | "meta-data") {
                        eprintln!("  Merging {} from {}", elem_name, aar_manifest_path.display());
                        let new = NewElement { name: aar_app_child.name(&aar_doc).parse().unwrap(), attrs: aar_app_child.attributes(&aar_doc).clone() };
                        app_elem.append_new_element(&mut host_doc, new);
                        // app_elem.append_new_element(document, new_element)
                    }
                }
            }
        }

        for child in aar_doc.root().children(&aar_doc) {
            if child.name(&aar_doc) == "uses-permission" {
                eprintln!("  Merging uses-permission from {}", aar_manifest_path.display());
                let new = NewElement { name: child.name(&aar_doc).parse().unwrap(), attrs: child.attributes(&aar_doc).clone() };
                manifest_element.append_new_element(&mut host_doc, new);
            }
        }
    }

    let merged_content = host_doc.to_string_pretty().replace("${applicationId}", &host_package);
    std::fs::write(output_manifest, merged_content)?;
    eprintln!("  Wrote merged manifest to: {}", output_manifest.display());

    Ok(())
}

async fn build_host_apk(
    sdk: &android_sdk::AndroidSdk,
    output_dir: &Path,
    api_level: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("🏗️  Building host APK...");

    let build_dir = output_dir.join("host-build");
    std::fs::create_dir_all(&build_dir)
        .map_err(|e| format!("Failed to create build directory {}: {}", build_dir.display(), e))?;

    // Get Android permissions from Cargo.toml
    let permissions = get_android_permissions();
    if !permissions.is_empty() {
        println!("📱 Adding {} permission(s) to manifest", permissions.len());
        for perm in &permissions {
            println!("   - {}", perm);
        }
    }

    // Resolve Maven dependencies
    let maven_deps = get_maven_dependencies();
    let resolved_artifacts = if !maven_deps.is_empty() {
        println!("📦 Resolving {} Maven dependencies...", maven_deps.len());
        let cache_dir = output_dir.join("maven-cache");
        let lock_file = std::path::Path::new("vampire.lock").to_path_buf();
        let resolver = maven::MavenResolver::new(cache_dir)?.with_lock_file(lock_file);
        let mut artifacts = resolver.resolve(&maven_deps).await?;
        artifacts.sort();
        println!("✅ Resolved {} artifact(s)", artifacts.len());

        let mut total_native_libs = 0;
        for artifact in &artifacts {
            println!("   - {}:{}:{}",
                artifact.coordinate.group_id,
                artifact.coordinate.artifact_id,
                artifact.coordinate.version
            );
            if !artifact.native_libs.is_empty() {
                total_native_libs += artifact.native_libs.len();
                eprintln!("     └─ {} native libraries", artifact.native_libs.len());
            }
        }

        if total_native_libs > 0 {
            println!("📚 Found {} native libraries across all artifacts", total_native_libs);
        }

        artifacts
    } else {
        Vec::new()
    };

    // Write embedded host template files
    let resources = get_android_resources();
    let components = get_manifest_components();
    host_templates::write_host_files(&build_dir, &permissions, &resources, &components)?;

    let gen_dir = build_dir.join("gen");
    let obj_dir = build_dir.join("obj");
    let libs_dir = build_dir.join("libs");

    std::fs::create_dir_all(&gen_dir)
        .map_err(|e| format!("Failed to create gen directory {}: {}", gen_dir.display(), e))?;
    std::fs::create_dir_all(&obj_dir)
        .map_err(|e| format!("Failed to create obj directory {}: {}", obj_dir.display(), e))?;
    std::fs::create_dir_all(&libs_dir)
        .map_err(|e| format!("Failed to create libs directory {}: {}", libs_dir.display(), e))?;

    let manifest = build_dir.join("AndroidManifest.xml");
    let res_dir = build_dir.join("res");
    let java_dir = build_dir.join("java");

    // Step 2.5: Build AAR resources as shared libraries (per BUILD.md)
    println!("🎨 Building AAR resources...");

    // Create shared ids.txt for stable resource IDs across all AARs and host app
    let shared_ids_txt = build_dir.join("ids.txt");
    std::fs::write(&shared_ids_txt, "")?;
    let shared_ids_txt = std::fs::canonicalize(&shared_ids_txt)?;  // Make absolute for aapt2

    let mut aar_flat_files = Vec::new();     // For APK resource merging
    let mut aar_packages = Vec::new();       // Package names for --extra-packages
    let mut aar_manifests = Vec::new();      // Manifest paths for merging

    for artifact in &resolved_artifacts {
        if artifact.is_aar && artifact.res_dir.is_some() && artifact.manifest_path.is_some() {
            // AAR root is where AndroidManifest.xml and res/ live
            let aar_root = artifact.jar_path.parent().unwrap();

            // Clean build directory
            let aar_build_dir = aar_root.join("build");
            if aar_build_dir.exists() {
                std::fs::remove_dir_all(&aar_build_dir)?;
            }

            println!("  Compiling {} resources...", artifact.coordinate);
            let flat_files = sdk.compile_aar_resources(aar_root).await?;

            aar_flat_files.extend(flat_files);

            // Collect package name if available
            if let Some(package_name) = &artifact.package_name {
                aar_packages.push(package_name.clone());
            }

            // Collect manifest for merging
            if let Some(manifest_path) = &artifact.manifest_path {
                aar_manifests.push(manifest_path.clone());
            }
        }
    }

    println!("Compiled {} AAR resource libraries with {} total .flat files",
             aar_packages.len(), aar_flat_files.len());

    // Step 2.6: Merge AAR manifests into host manifest
    println!("📝 Merging AAR manifests...");
    let merged_manifest = build_dir.join("AndroidManifest-merged.xml");
    merge_manifests(&manifest, &aar_manifests, &merged_manifest)?;

    // Step 2.7: Build host app resources (merge with AAR resources)
    println!("🎨 Generating host R.java...");
    let r_java_gen_dir = build_dir.join("gen");
    std::fs::create_dir_all(&r_java_gen_dir)?;

    sdk.generate_r_java_v2(&merged_manifest, &res_dir, &aar_flat_files, &aar_packages, &shared_ids_txt, &r_java_gen_dir, api_level).await?;

    // Collect ALL R.java files (host + AARs) generated by --extra-packages
    let mut all_r_java_files = Vec::new();
    sdk.find_r_java_files(&r_java_gen_dir, &mut all_r_java_files)?;

    eprintln!("Found {} R.java files to compile", all_r_java_files.len());

    let instrumentation_java = java_dir
        .join("com/vampire/host")
        .join("VampireInstrumentation.java");
    let test_runner_java = java_dir.join("com/vampire/loader").join("TestRunner.java");
    let test_metadata_java = java_dir
        .join("com/vampire/loader")
        .join("TestMetadata.java");

    let classpath: Vec<&Path> = resolved_artifacts.iter()
        .map(|a| a.jar_path.as_path())
        .collect();

    // Compile all R.java files + host Java files
    let mut java_files_to_compile: Vec<&Path> = all_r_java_files.iter().map(|p| p.as_path()).collect();
    java_files_to_compile.push(&instrumentation_java);
    java_files_to_compile.push(&test_runner_java);
    java_files_to_compile.push(&test_metadata_java);

    sdk.compile_java(
        &java_files_to_compile,
        &sdk.sdk_path
            .join("platforms")
            .join(format!("android-{}", api_level))
            .join("android.jar"),
        &classpath,
        &build_dir.join("obj"),
    )
    .await?;

    // Step 3: Convert to DEX (obj contains all R.class files now)
    let classes_dex = build_dir.join("classes.dex");
    let obj_dir = build_dir.join("obj");

    // DEX inputs: just Maven JARs (R.class files are in obj/ now)
    let dex_inputs: Vec<&Path> = classpath.clone();

    eprintln!("DEBUG: Converting to DEX with {} JAR inputs", dex_inputs.len());
    eprintln!("DEBUG:   obj dir: {} (contains all R.class files)", obj_dir.display());

    sdk.convert_to_dex(&[&obj_dir], &dex_inputs, &classes_dex, api_level)
        .await?;

    // Step 3.5: Organize native libraries by architecture
    let libs_dir = build_dir.join("lib");

    // Copy Maven dependency native libraries
    for artifact in &resolved_artifacts {
        for (arch, lib_path) in &artifact.native_libs {
            let target_dir = libs_dir.join(arch);
            std::fs::create_dir_all(&target_dir)
                .map_err(|e| format!("Failed to create lib/{} directory {}: {}", arch, target_dir.display(), e))?;

            let lib_name = lib_path.file_name().ok_or("Invalid library path")?;
            let target_path = target_dir.join(lib_name);

            std::fs::copy(lib_path, &target_path)
                .map_err(|e| format!("Failed to copy {} to {}: {}", lib_path.display(), target_path.display(), e))?;

            eprintln!("Copied Maven native library: {} -> {}", lib_path.display(), target_path.display());
        }
    }

    // Step 4: Package APK with aapt2 (merges host + AAR resources)
    let unsigned_apk = build_dir.join(format!("{}-unsigned.apk", APK_NAME));
    sdk.package_apk_v2(&merged_manifest, &res_dir, &aar_flat_files, &aar_packages, &shared_ids_txt, &classes_dex, &libs_dir, &unsigned_apk, api_level)
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
    std::fs::copy(&signed_apk, &final_apk)
        .map_err(|e| format!("Failed to copy {} to {}: {}", signed_apk.display(), final_apk.display(), e))?;

    println!("✅ Host APK created: {}", final_apk.display());
    Ok(())
}

async fn run_tests(device: Option<String>, force: bool, nocapture: bool, trace: bool, logcat_filters: Vec<String>, test_filter: Option<String>) {
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

    // Step 5: Run instrumentation tests
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

    // Build logcat filters: start with base filters, then add user-specified ones
    let mut filters: Vec<String> = if trace {
        vec![]  // No filtering with --trace
    } else if nocapture {
        vec!["TestRunner:*".to_string(), "*:F".to_string()]
    } else {
        vec!["TestRunner:I".to_string(), "*:F".to_string()]
    };

    // Add user-specified filters additively (ignored if --trace is set)
    if !trace {
        filters.extend(logcat_filters.iter().cloned());
    }

    // Build logcat command
    if filters.is_empty() {
        logcat_cmd.args(&["logcat", "-v", "threadtime"]);
    } else {
        logcat_cmd.args(&["logcat", "-v", "threadtime", "-s"]);
        for filter in &filters {
            logcat_cmd.arg(filter);
        }
    }

    logcat_cmd.stdout(std::process::Stdio::piped());
    logcat_cmd.stderr(std::process::Stdio::null());

    let mut child = logcat_cmd.spawn().ok();

    // Spawn a task to read and filter logcat output
    let logcat_handle = if let Some(ref mut child) = child {
        if let Some(stdout) = child.stdout.take() {
            let nocapture_flag = nocapture;
            let trace_flag = trace;
            Some(tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    // Skip logcat system messages
                    if line.starts_with("--------- beginning of") {
                        continue;
                    }

                    if trace_flag {
                        // Trace mode: print everything without filtering
                        println!("{}", line);
                    } else {
                        // threadtime format: MM-DD HH:MM:SS.mmm  PID  TID LEVEL TAG: message
                        // Extract priority level and message
                        if let Some(message_start) = line.find(": ") {
                            let message = &line[message_start + 2..];

                            // Find priority level (I, D, E, W, F, etc.) - it's before the tag
                            let is_info = line.contains(" I ") || line.contains(" I/");
                            let is_error = line.contains(" E ") || line.contains(" E/");
                            let is_fatal = line.contains(" F ") || line.contains(" F/");

                            if is_fatal {
                                // Fatal errors: always print to stderr
                                eprintln!("\x1b[31mFATAL:\x1b[0m {}", message);
                            } else if is_info {
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
    ]);

    // Add test filter if specified
    if let Some(filter) = &test_filter {
        test_args.extend_from_slice(&[
            "-e".to_string(),
            "test_filter".to_string(),
            filter.clone(),
        ]);
    }

    test_args.push(format!("{}/.{}", HOST_PACKAGE, INSTRUMENTATION_CLASS));

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

async fn show_dependencies() {
    println!("📦 Resolving Maven dependencies (dry-run)...\n");

    // Get Maven dependencies from Cargo.toml
    let maven_deps = get_maven_dependencies();

    if maven_deps.is_empty() {
        println!("No Maven dependencies configured in Cargo.toml");
        println!("\nAdd dependencies in [package.metadata.vampire.dependencies]:");
        println!("  \"org.chromium.net:cronet-api\" = \"119.6045.31\"");
        return;
    }

    // Create resolver
    let cache_dir = std::path::Path::new(OUTPUT_DIR).join("maven-cache");
    let lock_file = std::path::Path::new("vampire.lock").to_path_buf();
    let resolver = match maven::MavenResolver::new(cache_dir) {
        Ok(r) => r.with_lock_file(lock_file),
        Err(e) => {
            eprintln!("❌ Failed to create Maven resolver: {}", e);
            return;
        }
    };

    // Check if lock file exists
    if let Ok(Some(lock)) = resolver.read_lock() {
        println!("📄 Lock file status:");
        println!("   Version: {}", lock.version);
        println!("   Generated: {}", lock.metadata.generated_at);
        println!("   Artifacts: {}\n", lock.artifacts.len());
    }

    // Resolve dependencies (dry-run - only downloads POMs)
    let nodes = match resolver.resolve_dependencies_dry_run(&maven_deps).await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("❌ Failed to resolve dependencies: {}", e);
            return;
        }
    };

    // Print dependency tree
    resolver.print_dependency_tree(&nodes);

    // Detect potential conflicts (just warnings, no auto-fix)
    resolver.detect_conflicts(&nodes);

    // Display summary
    println!("\n✅ Resolved {} artifact(s)", nodes.len());
    for node in &nodes {
        println!("   - {}", node.coordinate);
    }
}

async fn update_dependencies() {
    println!("🔄 Updating Maven dependencies...\n");

    // Get Maven dependencies from Cargo.toml
    let maven_deps = get_maven_dependencies();

    if maven_deps.is_empty() {
        println!("No Maven dependencies configured in Cargo.toml");
        println!("\nAdd dependencies in [package.metadata.vampire.dependencies]:");
        println!("  \"org.chromium.net:cronet-api\" = \"119.6045.31\"");
        return;
    }

    // Create resolver with lock file
    let cache_dir = std::path::Path::new(OUTPUT_DIR).join("maven-cache");
    let lock_file = std::path::Path::new("vampire.lock").to_path_buf();
    let resolver = match maven::MavenResolver::new(cache_dir) {
        Ok(r) => r.with_lock_file(lock_file),
        Err(e) => {
            eprintln!("❌ Failed to create Maven resolver: {}", e);
            return;
        }
    };

    // Force update (ignore existing lock file)
    let artifacts = match resolver.resolve_with_lock(&maven_deps, true).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("❌ Failed to update dependencies: {}", e);
            return;
        }
    };

    println!("\n✅ Updated {} artifact(s)", artifacts.len());
    for artifact in &artifacts {
        println!("   - {}:{}:{}",
            artifact.coordinate.group_id,
            artifact.coordinate.artifact_id,
            artifact.coordinate.version
        );
    }
}
