use std::path::{Path, PathBuf};
use std::process::Output;
use tokio::process::Command;

#[derive(Clone)]
pub struct AndroidSdk {
    pub sdk_path: PathBuf,
    pub build_tools_version: String,
}

impl AndroidSdk {
    /// Find Android SDK from environment or default locations
    pub fn find() -> Result<Self, Box<dyn std::error::Error>> {
        let sdk_path = if let Ok(path) = std::env::var("ANDROID_SDK_ROOT") {
            path
        } else if let Ok(path) = std::env::var("ANDROID_HOME") {
            path
        } else {
            // Try default location (macOS: ~/Library/Android/sdk)
            let home = pathos::user::home_dir()?;
            let default_path = home.join("Library/Android/sdk");

            if default_path.exists() {
                default_path.to_string_lossy().to_string()
            } else {
                return Err("Android SDK not found. Set ANDROID_SDK_ROOT or ANDROID_HOME".into());
            }
        };

        let sdk_path = PathBuf::from(sdk_path);

        // Find build-tools version
        let build_tools_dir = sdk_path.join("build-tools");
        if !build_tools_dir.exists() {
            return Err("Android SDK build-tools not found".into());
        }

        // Use the highest version available, or fall back to 34.0.0
        let build_tools_version = std::fs::read_dir(&build_tools_dir)
            .map_err(|e| {
                format!(
                    "Failed to read build-tools directory {}: {}",
                    build_tools_dir.display(),
                    e
                )
            })?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Parse version to find highest
                name.split('.')
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .map(|_| name)
            })
            .max()
            .unwrap_or_else(|| "34.0.0".to_string());

        Ok(Self {
            sdk_path,
            build_tools_version,
        })
    }

    /// Get path to a build tool
    fn tool_path(&self, tool: &str) -> PathBuf {
        self.sdk_path
            .join("build-tools")
            .join(&self.build_tools_version)
            .join(tool)
    }

    /// Get path to platform android.jar
    fn android_jar(&self, api_level: u32) -> PathBuf {
        self.sdk_path
            .join("platforms")
            .join(format!("android-{}", api_level))
            .join("android.jar")
    }

    /// Run aapt command
    pub async fn run_aapt(&self, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
        let output = Command::new(self.tool_path("aapt"))
            .args(args)
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!("aapt failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        Ok(output)
    }

    /// Run javac command
    pub async fn run_javac(&self, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
        let output = Command::new("javac").args(args).output().await?;

        if !output.status.success() {
            return Err(
                format!("javac failed: {}", String::from_utf8_lossy(&output.stderr)).into(),
            );
        }

        Ok(output)
    }

    /// Run d8 command (try d8, fall back to dx)
    pub async fn run_d8(&self, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
        // Try d8 first
        let d8_path = self.tool_path("d8");
        if d8_path.exists() {
            eprintln!("Running d8: {}", d8_path.display());
            let output = Command::new(&d8_path)
                .args(args)
                .output()
                .await
                .map_err(|e| format!("Failed to execute d8 at {}: {}", d8_path.display(), e))?;

            if output.status.success() {
                return Ok(output);
            } else {
                eprintln!(
                    "d8 failed with stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                eprintln!("d8 stdout: {}", String::from_utf8_lossy(&output.stdout));
                return Err(
                    format!("d8 failed: {}", String::from_utf8_lossy(&output.stderr)).into(),
                );
            }
        }

        // Fall back to dx
        let dx_path = self.tool_path("dx");
        eprintln!("Falling back to dx: {}", dx_path.display());
        let output = Command::new(&dx_path)
            .args(args)
            .output()
            .await
            .map_err(|e| format!("Failed to execute dx at {}: {}", dx_path.display(), e))?;

        if !output.status.success() {
            return Err(format!("dx failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        Ok(output)
    }

    /// Run apksigner command
    pub async fn run_apksigner(&self, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
        let output = Command::new(self.tool_path("apksigner"))
            .args(args)
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "apksigner failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        Ok(output)
    }

    /// Run zipalign command
    pub async fn run_zipalign(&self, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
        let output = Command::new(self.tool_path("zipalign"))
            .args(args)
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "zipalign failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        Ok(output)
    }

    /// Run adb command
    pub async fn run_adb(&self, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
        let adb_path = self.sdk_path.join("platform-tools").join("adb");
        let output = Command::new(adb_path).args(args).output().await?;

        if !output.status.success() {
            return Err(format!("adb failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        Ok(output)
    }

    /// Generate R.java from resources
    /// Generate R.java using aapt2 (merges host + AAR resources)
    /// Uses --extra-packages to generate R.java for all AAR packages with correct IDs
    pub async fn generate_r_java_v2(
        &self,
        manifest: &Path,
        res_dir: &Path,
        aar_flat_files: &[PathBuf],
        aar_packages: &[String],
        shared_ids_txt: &Path,
        output_dir: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Generating R.java with aapt2...");

        let build_dir = output_dir.parent().unwrap();
        let compiled_res_dir = build_dir.join("compiled-res-rjava");
        let compiled_res_zip = build_dir.join("compiled-res-rjava.zip");
        std::fs::create_dir_all(&compiled_res_dir)?;
        std::fs::create_dir_all(output_dir)?;

        // Step 1: Compile host resources to .flat files
        let aapt2 = self.tool_path("aapt2");
        let output = tokio::process::Command::new(&aapt2)
            .arg("compile")
            .arg("--dir")
            .arg(res_dir)
            .arg("-o")
            .arg(&compiled_res_zip)
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "aapt2 compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        // Step 2: Extract .flat files
        let zip_file = std::fs::File::open(&compiled_res_zip)?;
        let mut archive = zip::ZipArchive::new(zip_file)?;

        let mut host_flat_files = Vec::new();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let filename = file.name().to_string();
            let outpath = compiled_res_dir.join(&filename);

            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;

            host_flat_files.push(outpath);
        }

        // Step 3: Use shared ids.txt (already populated by AAR builds)

        // Step 4: Run aapt2 link to generate R.java (don't need full APK, just R.java)
        // Use API 36 when merging with AAR resources (same as AAR builds)
        let android_jar = self.android_jar(36);
        let dummy_apk = build_dir.join("dummy-rjava.apk");

        let mut cmd = tokio::process::Command::new(&aapt2);
        cmd.arg("link")
            .arg("-I")
            .arg(&android_jar)
            .arg("-o")
            .arg(&dummy_apk)
            .arg("--manifest")
            .arg(manifest)
            .arg("--java")
            .arg(output_dir)
            .arg("--auto-add-overlay")
            .arg("--stable-ids")
            .arg(shared_ids_txt)
            .arg("--emit-ids")
            .arg(shared_ids_txt);

        // Add --extra-packages for all AAR packages
        // This generates R.java for each AAR package with IDs matching the merged resources
        if !aar_packages.is_empty() {
            let extra_packages = aar_packages.join(":");
            cmd.arg("--extra-packages").arg(&extra_packages);
            eprintln!("  Generating R.java for packages: {}", extra_packages);
        }

        // Add host .flat files
        for flat_file in &host_flat_files {
            cmd.arg("-R").arg(flat_file);
        }

        // Add AAR .flat files
        for flat_file in aar_flat_files {
            cmd.arg("-R").arg(flat_file);
        }

        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("aapt2 link stderr: {}", stderr);
            return Err(format!("aapt2 link failed: {}", stderr).into());
        }

        // Clean up dummy APK
        let _ = std::fs::remove_file(&dummy_apk);

        Ok(())
    }

    /// Compile Java sources to .class files
    pub async fn compile_java(
        &self,
        sources: &[&Path],
        bootclasspath: &Path,
        classpath: &[&Path],
        output_dir: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Compiling Java sources...");

        let classpath_str = classpath
            .iter()
            .map(|p| p.to_str().unwrap())
            .collect::<Vec<_>>()
            .join(":");

        let mut args = vec![
            "-source",
            "1.8",
            "-target",
            "1.8",
            "-bootclasspath",
            bootclasspath.to_str().unwrap(),
        ];

        // Add classpath if provided
        if !classpath.is_empty() {
            args.push("-classpath");
            args.push(&classpath_str);
        }

        args.push("-d");
        args.push(output_dir.to_str().unwrap());

        // Add source files
        for source in sources {
            args.push(source.to_str().unwrap());
        }

        self.run_javac(&args).await?;

        Ok(())
    }

    /// Convert .class files to .dex (supports multiple input sources)
    pub async fn convert_to_dex(
        &self,
        class_dirs: &[&Path],
        jars: &[&Path],
        output_file: &Path,
        api_level: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Converting to DEX...");

        // Try d8 first (modern)
        let d8_path = self.tool_path("d8");
        if d8_path.exists() {
            let mut args = vec![
                "--output".to_string(),
                output_file.parent().unwrap().to_str().unwrap().to_string(),
            ];

            // Add android.jar as library for desugaring
            let android_jar = self.android_jar(api_level);
            args.push("--lib".to_string());
            args.push(android_jar.to_str().unwrap().to_string());

            // Add all class directories
            for dir in class_dirs {
                let mut class_paths = Vec::new();
                self.find_class_files(dir, &mut class_paths)?;
                for path in class_paths {
                    args.push(path);
                }
            }

            // Add all JARs
            for jar in jars {
                args.push(jar.to_str().unwrap().to_string());
            }

            let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            eprintln!("Running d8 with args: {:?}", args_str);
            self.run_d8(&args_str).await?;

            // d8 creates classes.dex, rename if needed
            let classes_dex = output_file.parent().unwrap().join("classes.dex");
            eprintln!("Expected DEX output: {}", classes_dex.display());
            eprintln!("Target DEX location: {}", output_file.display());

            if classes_dex.exists() && classes_dex != output_file {
                std::fs::rename(&classes_dex, output_file).map_err(|e| {
                    format!(
                        "Failed to rename {} to {}: {}",
                        classes_dex.display(),
                        output_file.display(),
                        e
                    )
                })?;
            }
        } else {
            // Fall back to dx - just use first class dir for simplicity
            if let Some(first_dir) = class_dirs.first() {
                self.run_d8(&[
                    "--dex",
                    &format!("--output={}", output_file.to_str().unwrap()),
                    first_dir.to_str().unwrap(),
                ])
                .await?;
            }
        }

        // Verify the DEX file was created
        if !output_file.exists() {
            return Err(format!(
                "DEX file was not created at expected location: {}",
                output_file.display()
            )
            .into());
        }

        eprintln!("DEX file created successfully: {}", output_file.display());
        Ok(())
    }

    /// Helper to recursively find .class files
    fn find_class_files(
        &self,
        dir: &Path,
        results: &mut Vec<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("DEBUG: Searching for .class files in: {}", dir.display());
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)
                .map_err(|e| format!("Failed to read directory {}: {}", dir.display(), e))?
            {
                let entry = entry.map_err(|e| {
                    format!("Failed to read directory entry in {}: {}", dir.display(), e)
                })?;
                let path = entry.path();
                if path.is_dir() {
                    self.find_class_files(&path, results)?;
                } else if path.extension().and_then(|s| s.to_str()) == Some("class") {
                    eprintln!("DEBUG: Found .class file: {}", path.display());
                    results.push(path.to_str().unwrap().to_string());
                }
            }
        } else {
            eprintln!(
                "DEBUG: Path is not a directory or doesn't exist: {}",
                dir.display()
            );
        }
        eprintln!(
            "DEBUG: Total .class files found in {}: {}",
            dir.display(),
            results.len()
        );
        Ok(())
    }

    /// Compile AAR resources to .flat files only (no R.java generation)
    /// R.java will be generated later in final APK build to ensure correct IDs
    pub async fn compile_aar_resources(
        &self,
        aar_root: &Path,
    ) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        // All paths will be relative to aar_root
        // aar_root contains: AndroidManifest.xml, res/, and we create build/

        let build_dir = aar_root.join("build");
        let build_res = build_dir.join("res");
        std::fs::create_dir_all(&build_res)?;

        let aapt2 = self.tool_path("aapt2");
        let res_zip_temp = build_dir.join("res.zip");

        // Compile AAR resources to .flat files
        eprintln!("  Compiling AAR resources to .flat files...");
        let output = tokio::process::Command::new(&aapt2)
            .current_dir(aar_root)
            .arg("compile")
            .arg("--dir")
            .arg("res")
            .arg("-o")
            .arg("build/res.zip")
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "aapt2 compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        // Extract .flat files from res.zip
        let zip_file = std::fs::File::open(&res_zip_temp)?;
        let mut archive = zip::ZipArchive::new(zip_file)?;
        let mut flat_files_absolute = Vec::new();

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let filename = file.name().to_string();
            let outpath = build_res.join(&filename);

            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;

            flat_files_absolute.push(outpath);
        }

        eprintln!("  → {} .flat files compiled", flat_files_absolute.len());

        Ok(flat_files_absolute)
    }

    /// Package APK (old unused function)
    #[allow(dead_code)]
    async fn compile_aar_resources_old(
        &self,
        res_dir: &Path,
        output_dir: &Path,
    ) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        eprintln!(
            "DEBUG: compile_aar_resources called for: {}",
            res_dir.display()
        );
        let aapt2 = self.tool_path("aapt2");
        let mut compiled_files = Vec::new();

        // aapt2 compile needs to process each resource file
        // Find all resource files recursively
        let res_files = std::fs::read_dir(res_dir).map_err(|e| {
            format!(
                "Failed to read resource directory {}: {}",
                res_dir.display(),
                e
            )
        })?;

        for entry in res_files {
            let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
            let path = entry.path();
            eprintln!("DEBUG:   Found entry: {}", path.display());

            if path.is_dir() {
                eprintln!("DEBUG:   Processing subdirectory: {}", path.display());
                // Process files in subdirectory (e.g., values/, layout/, etc.)
                let subdir_files = std::fs::read_dir(&path).map_err(|e| {
                    format!("Failed to read subdirectory {}: {}", path.display(), e)
                })?;

                for subentry in subdir_files {
                    let subentry =
                        subentry.map_err(|e| format!("Failed to read subdir entry: {}", e))?;
                    let res_file = subentry.path();
                    eprintln!("DEBUG:     Found resource file: {}", res_file.display());

                    if res_file.is_file() {
                        eprintln!("DEBUG:     Compiling with aapt2: {}", res_file.display());
                        // Compile this resource file
                        // aapt2 compile res/values/values.xml -o output_dir/
                        let output = tokio::process::Command::new(&aapt2)
                            .arg("compile")
                            .arg(&res_file)
                            .arg("-o")
                            .arg(output_dir)
                            .arg("-v")
                            .output()
                            .await
                            .map_err(|e| format!("Failed to run aapt2 compile: {}", e))?;

                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);

                        if !stdout.is_empty() {
                            eprintln!("DEBUG:     aapt2 stdout: {}", stdout);
                        }
                        if !stderr.is_empty() {
                            eprintln!("DEBUG:     aapt2 stderr: {}", stderr);
                        }

                        if !output.status.success() {
                            eprintln!(
                                "DEBUG:     aapt2 compile FAILED with status: {}",
                                output.status
                            );
                            return Err(format!(
                                "aapt2 compile failed for {}: {}",
                                res_file.display(),
                                stderr
                            )
                            .into());
                        }

                        // Compiled file will be named <resource-name>.flat
                        let flat_name = res_file
                            .file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| format!("{}.zip", n.replace(".xml", "")))
                            .ok_or("Invalid resource filename")?;

                        let compiled_file = output_dir.join(flat_name);
                        eprintln!(
                            "DEBUG:     Looking for compiled file: {}",
                            compiled_file.display()
                        );
                        if compiled_file.exists() {
                            eprintln!("DEBUG:     SUCCESS - compiled file exists!");
                            compiled_files.push(compiled_file);
                        } else {
                            eprintln!("DEBUG:     WARNING - compiled file does NOT exist!");
                        }
                    }
                }
            }
        }

        Ok(compiled_files)
    }

    pub fn find_r_java_files(
        &self,
        dir: &Path,
        files: &mut Vec<PathBuf>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.find_r_java_files(&path, files)?;
            } else if path.extension().and_then(|s| s.to_str()) == Some("java") {
                files.push(path);
            }
        }
        Ok(())
    }

    /// Package APK using aapt2 link (merges host + AAR resources)
    pub async fn package_apk_v2(
        &self,
        manifest: &Path,
        res_dir: &Path,
        aar_flat_files: &[PathBuf],
        aar_packages: &[String],
        shared_ids_txt: &Path,
        dex_file: &Path,
        libs_dir: &Path,
        output_apk: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Packaging APK with aapt2...");

        // Verify inputs exist
        if !manifest.exists() {
            return Err(format!("Manifest file not found: {}", manifest.display()).into());
        }
        if !res_dir.exists() {
            return Err(format!("Resources directory not found: {}", res_dir.display()).into());
        }
        if !dex_file.exists() {
            return Err(format!("DEX file not found: {}", dex_file.display()).into());
        }

        let build_dir = output_apk.parent().unwrap();
        let compiled_res_dir = build_dir.join("compiled-res");
        let compiled_res_zip = build_dir.join("compiled-res.zip");
        std::fs::create_dir_all(&compiled_res_dir)?;

        // Step 1: Compile host resources to .flat files
        eprintln!("  Compiling host resources...");
        let aapt2 = self.tool_path("aapt2");
        let output = tokio::process::Command::new(&aapt2)
            .arg("compile")
            .arg("--dir")
            .arg(res_dir)
            .arg("-o")
            .arg(&compiled_res_zip)
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "aapt2 compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        // Step 2: Extract .flat files
        let zip_file = std::fs::File::open(&compiled_res_zip)?;
        let mut archive = zip::ZipArchive::new(zip_file)?;

        let mut host_flat_files = Vec::new();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let filename = file.name().to_string();
            let outpath = compiled_res_dir.join(&filename);

            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;

            host_flat_files.push(outpath);
        }

        // Step 3: Use shared ids.txt (already populated by AAR builds and host R.java generation)

        // Step 4: Run aapt2 link with host .flat files + AAR resources
        eprintln!("  Linking APK with AAR resources...");
        // Use API 36 when merging with AAR resources (same as AAR builds)
        let android_jar = self.android_jar(36);
        let proto_apk = build_dir.join("proto.apk");

        let mut cmd = tokio::process::Command::new(&aapt2);
        cmd.arg("link")
            .arg("-I")
            .arg(&android_jar)
            .arg("-o")
            .arg(&proto_apk)
            .arg("--manifest")
            .arg(manifest)
            .arg("--auto-add-overlay")
            .arg("--stable-ids")
            .arg(shared_ids_txt)
            .arg("--emit-ids")
            .arg(shared_ids_txt);

        // Add --extra-packages (not strictly needed for APK, but ensures consistency)
        if !aar_packages.is_empty() {
            let extra_packages = aar_packages.join(":");
            cmd.arg("--extra-packages").arg(&extra_packages);
        }

        // Add host .flat files
        for flat_file in &host_flat_files {
            cmd.arg("-R").arg(flat_file);
        }

        // Add AAR .flat files
        for flat_file in aar_flat_files {
            cmd.arg("-R").arg(flat_file);
        }

        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("aapt2 link stderr: {}", stderr);
            return Err(format!("aapt2 link failed: {}", stderr).into());
        }

        // Step 5: Add DEX to APK
        self.run_aapt(&[
            "add",
            "-k",
            proto_apk.to_str().unwrap(),
            dex_file.to_str().unwrap(),
        ])
        .await?;

        // Step 6: Add native libraries to APK if libs_dir exists
        if libs_dir.exists() && libs_dir.is_dir() {
            use std::io::{Read, Write};
            use zip::write::FileOptions;

            let apk_file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&proto_apk)
                .map_err(|e| format!("Failed to open APK for writing: {}", e))?;

            let mut zip = zip::ZipWriter::new_append(apk_file)
                .map_err(|e| format!("Failed to open APK as ZIP: {}", e))?;

            let options = FileOptions::<()>::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(0o755);

            for arch_entry in std::fs::read_dir(libs_dir).map_err(|e| {
                format!(
                    "Failed to read libs directory {}: {}",
                    libs_dir.display(),
                    e
                )
            })? {
                let arch_entry = arch_entry
                    .map_err(|e| format!("Failed to read arch directory entry: {}", e))?;
                let arch_dir = arch_entry.path();

                if arch_dir.is_dir() {
                    let arch_name = arch_dir.file_name().unwrap().to_str().unwrap();

                    for lib_entry in std::fs::read_dir(&arch_dir)
                        .map_err(|e| format!("Failed to read lib/{} directory: {}", arch_name, e))?
                    {
                        let lib_entry = lib_entry
                            .map_err(|e| format!("Failed to read library entry: {}", e))?;
                        let lib_path = lib_entry.path();

                        if lib_path.extension().and_then(|s| s.to_str()) == Some("so") {
                            let lib_name = lib_path.file_name().unwrap().to_str().unwrap();
                            let zip_path = format!("lib/{}/{}", arch_name, lib_name);

                            eprintln!("Adding native library to APK: {}", zip_path);

                            zip.start_file(&zip_path, options)
                                .map_err(|e| format!("Failed to add {} to APK: {}", zip_path, e))?;

                            let mut lib_file = std::fs::File::open(&lib_path).map_err(|e| {
                                format!("Failed to open {}: {}", lib_path.display(), e)
                            })?;

                            let mut buffer = Vec::new();
                            lib_file.read_to_end(&mut buffer).map_err(|e| {
                                format!("Failed to read {}: {}", lib_path.display(), e)
                            })?;

                            zip.write_all(&buffer).map_err(|e| {
                                format!("Failed to write {} to APK: {}", zip_path, e)
                            })?;
                        }
                    }
                }
            }

            zip.finish()
                .map_err(|e| format!("Failed to finalize APK: {}", e))?;
        }

        // Step 7: Rename to final output
        std::fs::rename(&proto_apk, output_apk)?;

        Ok(())
    }

    /// Sign APK with debug keystore using apksigner (v2 signature)
    pub async fn sign_apk(
        &self,
        input_apk: &Path,
        output_apk: &Path,
        keystore: &Path,
        keystore_pass: &str,
        alias: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Signing APK...");

        // Create debug keystore if it doesn't exist
        if !keystore.exists() {
            self.create_debug_keystore(keystore, keystore_pass).await?;
        }

        self.run_apksigner(&[
            "sign",
            "--ks",
            keystore.to_str().unwrap(),
            "--ks-pass",
            &format!("pass:{}", keystore_pass),
            "--ks-key-alias",
            alias,
            "--out",
            output_apk.to_str().unwrap(),
            input_apk.to_str().unwrap(),
        ])
        .await?;

        Ok(())
    }

    /// Create debug keystore
    async fn create_debug_keystore(
        &self,
        keystore: &Path,
        password: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Creating debug keystore...");

        // Ensure parent directory exists
        if let Some(parent) = keystore.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create keystore directory {}: {}",
                    parent.display(),
                    e
                )
            })?;
        }

        let output = Command::new("keytool")
            .args(&[
                "-genkey",
                "-v",
                "-keystore",
                keystore.to_str().unwrap(),
                "-storepass",
                password,
                "-alias",
                "androiddebugkey",
                "-keypass",
                password,
                "-keyalg",
                "RSA",
                "-keysize",
                "2048",
                "-validity",
                "10000",
                "-dname",
                "CN=Android Debug,O=Android,C=US",
            ])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "keytool failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        Ok(())
    }

    /// Align APK
    pub async fn align_apk(
        &self,
        input_apk: &Path,
        output_apk: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Aligning APK...");

        self.run_zipalign(&[
            "-f",
            "4",
            input_apk.to_str().unwrap(),
            output_apk.to_str().unwrap(),
        ])
        .await?;

        Ok(())
    }
}
