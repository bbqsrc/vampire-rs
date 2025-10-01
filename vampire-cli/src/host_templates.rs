use std::path::Path;

pub const ANDROID_MANIFEST: &str = include_str!("../host/src/main/AndroidManifest.xml");
pub const STRINGS_XML: &str = include_str!("../host/src/main/res/values/strings.xml");
pub const VAMPIRE_INSTRUMENTATION: &str =
    include_str!("../host/src/main/java/com/vampire/host/VampireInstrumentation.java");
pub const TEST_RUNNER: &str =
    include_str!("../host/src/main/java/com/vampire/loader/TestRunner.java");
pub const TEST_METADATA: &str =
    include_str!("../host/src/main/java/com/vampire/loader/TestMetadata.java");

pub fn write_host_files(build_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // Create directory structure
    let manifest_path = build_dir.join("AndroidManifest.xml");
    let res_dir = build_dir.join("res/values");
    let java_host_dir = build_dir.join("java/com/vampire/host");
    let java_loader_dir = build_dir.join("java/com/vampire/loader");

    std::fs::create_dir_all(&res_dir)?;
    std::fs::create_dir_all(&java_host_dir)?;
    std::fs::create_dir_all(&java_loader_dir)?;

    // Write files
    std::fs::write(&manifest_path, ANDROID_MANIFEST)?;
    std::fs::write(res_dir.join("strings.xml"), STRINGS_XML)?;
    std::fs::write(
        java_host_dir.join("VampireInstrumentation.java"),
        VAMPIRE_INSTRUMENTATION,
    )?;
    std::fs::write(java_loader_dir.join("TestRunner.java"), TEST_RUNNER)?;
    std::fs::write(java_loader_dir.join("TestMetadata.java"), TEST_METADATA)?;

    Ok(())
}
