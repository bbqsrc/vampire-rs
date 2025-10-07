use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Simple configuration - just call this in your build.rs
pub fn configure() {
    Builder::new().configure();
}

/// Advanced configuration builder
pub struct Builder {
    java_sources: Vec<PathBuf>,
    java_dir: Option<PathBuf>,
    target_sdk: u32,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            java_sources: Vec::new(),
            java_dir: Some(PathBuf::from("java")),
            target_sdk: 30,
        }
    }

    /// Add a specific Java source file to compile
    pub fn java_source(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.java_sources.push(path.as_ref().to_path_buf());
        self
    }

    /// Set the directory containing Java sources (default: "java/")
    pub fn java_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.java_dir = Some(dir.as_ref().to_path_buf());
        self
    }

    /// Set the target SDK version (default: 30)
    pub fn target_sdk(&mut self, sdk: u32) -> &mut Self {
        self.target_sdk = sdk;
        self
    }

    /// Run the configuration
    pub fn configure(&self) {
        // 1. Always emit cfg(vampire) to enable conditional compilation
        println!("cargo:rustc-cfg=vampire");

        // 2. Check if building for Android
        let target = env::var("TARGET").unwrap_or_default();
        if !target.contains("android") {
            return;
        }

        // 3. Find Java sources
        let java_sources = if self.java_sources.is_empty() {
            self.find_java_sources()
        } else {
            self.java_sources.clone()
        };

        if java_sources.is_empty() {
            return;
        }

        // 4. Set rerun triggers
        if let Some(ref java_dir) = self.java_dir {
            println!("cargo:rerun-if-changed={}", java_dir.display());
        }
        println!("cargo:rerun-if-changed=Cargo.toml");

        // 5. Find javac
        let javac = match find_javac() {
            Some(j) => j,
            None => {
                eprintln!("Warning: javac not found. Please install JDK and set JAVA_HOME");
                eprintln!("Java sources will not be compiled for Android target");
                return;
            }
        };

        // 6. Compile Java sources
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let classes_dir = out_dir.join("classes");
        fs::create_dir_all(&classes_dir).expect("Failed to create classes directory");

        if let Err(e) = self.compile_java(&javac, &java_sources, &classes_dir) {
            panic!("Failed to compile Java sources: {}", e);
        }

        // 7. Convert .class files to DEX
        let dex_output = out_dir.join("classes.dex");
        if let Err(e) = self.convert_to_dex(&classes_dir, &dex_output) {
            panic!("Failed to convert to DEX: {}", e);
        }

        println!(
            "cargo:warning=Compiled {} Java source(s) and generated DEX at {}",
            java_sources.len(),
            dex_output.display()
        );
    }

    fn find_java_sources(&self) -> Vec<PathBuf> {
        let Some(ref java_dir) = self.java_dir else {
            return Vec::new();
        };

        if !java_dir.exists() {
            return Vec::new();
        }

        let mut sources = Vec::new();
        if let Err(e) = find_java_files_recursive(java_dir, &mut sources) {
            eprintln!(
                "Warning: Failed to scan Java directory {}: {}",
                java_dir.display(),
                e
            );
        }

        sources
    }

    fn compile_java(
        &self,
        javac: &Path,
        sources: &[PathBuf],
        out_dir: &Path,
    ) -> Result<(), String> {
        let mut cmd = Command::new(javac);
        cmd.arg("-d")
            .arg(out_dir)
            .arg("-source")
            .arg("8")
            .arg("-target")
            .arg("8");

        // Add sourcepath if java_dir is set
        if let Some(ref java_dir) = self.java_dir {
            cmd.arg("-sourcepath").arg(java_dir);
        }

        cmd.args(sources);

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run javac: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "javac failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    fn convert_to_dex(&self, classes_dir: &Path, dex_output: &Path) -> Result<(), String> {
        // Find d8 tool and Android SDK
        let d8_path = find_d8().ok_or_else(|| {
            "d8 not found. Please set ANDROID_SDK_ROOT or ANDROID_HOME environment variable"
                .to_string()
        })?;

        // Find android.jar for the target SDK
        let android_jar = find_android_jar(self.target_sdk)
            .ok_or_else(|| format!("android.jar not found for API level {}", self.target_sdk))?;

        // Collect all .class files
        let mut class_files = Vec::new();
        find_class_files_recursive(classes_dir, &mut class_files)
            .map_err(|e| format!("Failed to scan for class files: {}", e))?;

        if class_files.is_empty() {
            return Err("No .class files found to convert to DEX".to_string());
        }

        // Run d8 with android.jar as library for desugaring
        let mut cmd = Command::new(&d8_path);
        cmd.arg("--output")
            .arg(dex_output.parent().unwrap())
            .arg("--lib")
            .arg(&android_jar)
            .args(&class_files);

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run d8: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "d8 failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // d8 creates classes.dex in the output directory
        let generated_dex = dex_output.parent().unwrap().join("classes.dex");
        if generated_dex != *dex_output && generated_dex.exists() {
            fs::rename(&generated_dex, dex_output)
                .map_err(|e| format!("Failed to rename DEX file: {}", e))?;
        }

        Ok(())
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

fn find_java_files_recursive(dir: &Path, sources: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            find_java_files_recursive(&path, sources)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("java") {
            sources.push(path);
        }
    }

    Ok(())
}

fn find_class_files_recursive(dir: &Path, class_files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            find_class_files_recursive(&path, class_files)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("class") {
            class_files.push(path);
        }
    }

    Ok(())
}

fn find_javac() -> Option<PathBuf> {
    // Try JAVA_HOME first
    if let Ok(java_home) = env::var("JAVA_HOME") {
        let javac = PathBuf::from(java_home).join("bin").join("javac");
        if javac.exists() {
            return Some(javac);
        }
    }

    // Try PATH
    if let Ok(output) = Command::new("which").arg("javac").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }

    // Try common locations on macOS
    let macos_java = Path::new("/usr/libexec/java_home");
    if macos_java.exists() {
        if let Ok(output) = Command::new(macos_java).output() {
            if output.status.success() {
                let java_home = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let javac = PathBuf::from(java_home).join("bin").join("javac");
                if javac.exists() {
                    return Some(javac);
                }
            }
        }
    }

    None
}

fn find_d8() -> Option<PathBuf> {
    // Try ANDROID_SDK_ROOT first (official)
    if let Ok(sdk_root) = env::var("ANDROID_SDK_ROOT") {
        let d8 = find_d8_in_sdk(&PathBuf::from(sdk_root));
        if d8.is_some() {
            return d8;
        }
    }

    // Try ANDROID_HOME (deprecated but still common)
    if let Ok(android_home) = env::var("ANDROID_HOME") {
        let d8 = find_d8_in_sdk(&PathBuf::from(android_home));
        if d8.is_some() {
            return d8;
        }
    }

    // Try common locations
    let common_paths = vec![
        PathBuf::from(env::var("HOME").unwrap_or_default()).join("Android/Sdk"),
        PathBuf::from(env::var("HOME").unwrap_or_default()).join("Library/Android/sdk"),
        PathBuf::from("/usr/local/android-sdk"),
    ];

    for sdk_path in common_paths {
        let d8 = find_d8_in_sdk(&sdk_path);
        if d8.is_some() {
            return d8;
        }
    }

    None
}

fn find_d8_in_sdk(sdk_root: &Path) -> Option<PathBuf> {
    let build_tools_dir = sdk_root.join("build-tools");
    if !build_tools_dir.exists() {
        return None;
    }

    // Find the latest build-tools version
    let mut versions = Vec::new();
    if let Ok(entries) = fs::read_dir(&build_tools_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.path().is_dir() {
                versions.push(entry.path());
            }
        }
    }

    // Sort by name (version numbers) in descending order
    versions.sort_by(|a, b| {
        b.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .cmp(a.file_name().unwrap().to_str().unwrap())
    });

    // Find d8 in the latest version
    for version_dir in versions {
        let d8_path = version_dir.join("d8");
        if d8_path.exists() {
            return Some(d8_path);
        }
    }

    None
}

fn find_android_jar(api_level: u32) -> Option<PathBuf> {
    // Try ANDROID_SDK_ROOT first (official)
    if let Ok(sdk_root) = env::var("ANDROID_SDK_ROOT") {
        let android_jar = PathBuf::from(sdk_root)
            .join("platforms")
            .join(format!("android-{}", api_level))
            .join("android.jar");
        if android_jar.exists() {
            return Some(android_jar);
        }
    }

    // Try ANDROID_HOME (deprecated but still common)
    if let Ok(android_home) = env::var("ANDROID_HOME") {
        let android_jar = PathBuf::from(android_home)
            .join("platforms")
            .join(format!("android-{}", api_level))
            .join("android.jar");
        if android_jar.exists() {
            return Some(android_jar);
        }
    }

    // Try common locations
    let common_paths = vec![
        PathBuf::from(env::var("HOME").unwrap_or_default()).join("Android/Sdk"),
        PathBuf::from(env::var("HOME").unwrap_or_default()).join("Library/Android/sdk"),
        PathBuf::from("/usr/local/android-sdk"),
    ];

    for sdk_path in common_paths {
        let android_jar = sdk_path
            .join("platforms")
            .join(format!("android-{}", api_level))
            .join("android.jar");
        if android_jar.exists() {
            return Some(android_jar);
        }
    }

    None
}
