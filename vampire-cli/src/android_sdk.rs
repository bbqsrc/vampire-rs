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
        let build_tools_version = std::fs::read_dir(&build_tools_dir)?
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
            let output = Command::new(d8_path).args(args).output().await?;

            if output.status.success() {
                return Ok(output);
            }
        }

        // Fall back to dx
        let dx_path = self.tool_path("dx");
        let output = Command::new(dx_path).args(args).output().await?;

        if !output.status.success() {
            return Err(
                format!("d8/dx failed: {}", String::from_utf8_lossy(&output.stderr)).into(),
            );
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
    pub async fn generate_r_java(
        &self,
        manifest: &Path,
        res_dir: &Path,
        output_dir: &Path,
        api_level: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Generating R.java...");

        self.run_aapt(&[
            "package",
            "-f",
            "-m",
            "-J",
            output_dir.to_str().unwrap(),
            "-M",
            manifest.to_str().unwrap(),
            "-S",
            res_dir.to_str().unwrap(),
            "-I",
            self.android_jar(api_level).to_str().unwrap(),
        ])
        .await?;

        Ok(())
    }

    /// Compile Java sources to .class files
    pub async fn compile_java(
        &self,
        sources: &[&Path],
        bootclasspath: &Path,
        output_dir: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Compiling Java sources...");

        let mut args = vec![
            "-source",
            "1.8",
            "-target",
            "1.8",
            "-bootclasspath",
            bootclasspath.to_str().unwrap(),
            "-d",
            output_dir.to_str().unwrap(),
        ];

        // Add source files
        for source in sources {
            args.push(source.to_str().unwrap());
        }

        self.run_javac(&args).await?;

        Ok(())
    }

    /// Convert .class files to .dex
    pub async fn convert_to_dex(
        &self,
        class_files: &Path,
        output_file: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Converting to DEX...");

        // Try d8 first (modern)
        let d8_path = self.tool_path("d8");
        if d8_path.exists() {
            // Find all .class files
            let mut class_paths = Vec::new();
            self.find_class_files(class_files, &mut class_paths)?;

            let mut args = vec![
                "--output".to_string(),
                output_file.parent().unwrap().to_str().unwrap().to_string(),
            ];
            for path in class_paths {
                args.push(path);
            }

            let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            self.run_d8(&args_str).await?;

            // d8 creates classes.dex, rename if needed
            let classes_dex = output_file.parent().unwrap().join("classes.dex");
            if classes_dex.exists() && classes_dex != output_file {
                std::fs::rename(&classes_dex, output_file)?;
            }
        } else {
            // Fall back to dx
            self.run_d8(&[
                "--dex",
                &format!("--output={}", output_file.to_str().unwrap()),
                class_files.to_str().unwrap(),
            ])
            .await?;
        }

        Ok(())
    }

    /// Helper to recursively find .class files
    fn find_class_files(
        &self,
        dir: &Path,
        results: &mut Vec<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    self.find_class_files(&path, results)?;
                } else if path.extension().and_then(|s| s.to_str()) == Some("class") {
                    results.push(path.to_str().unwrap().to_string());
                }
            }
        }
        Ok(())
    }

    /// Package APK
    pub async fn package_apk(
        &self,
        manifest: &Path,
        res_dir: &Path,
        dex_file: &Path,
        output_apk: &Path,
        api_level: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("  Packaging APK...");

        // First create APK with resources
        self.run_aapt(&[
            "package",
            "-f",
            "-M",
            manifest.to_str().unwrap(),
            "-S",
            res_dir.to_str().unwrap(),
            "-I",
            self.android_jar(api_level).to_str().unwrap(),
            "-F",
            output_apk.to_str().unwrap(),
        ])
        .await?;

        // Then add DEX file to the APK
        self.run_aapt(&[
            "add",
            "-k",
            output_apk.to_str().unwrap(),
            dex_file.to_str().unwrap(),
        ])
        .await?;

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
            std::fs::create_dir_all(parent)?;
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
