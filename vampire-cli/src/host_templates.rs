use std::collections::HashMap;
use std::path::Path;

pub const STRINGS_XML: &str = include_str!("../host/src/main/res/values/strings.xml");
pub const VAMPIRE_INSTRUMENTATION: &str =
    include_str!("../host/src/main/java/com/vampire/host/VampireInstrumentation.java");
pub const TEST_RUNNER: &str =
    include_str!("../host/src/main/java/com/vampire/loader/TestRunner.java");
pub const TEST_METADATA: &str =
    include_str!("../host/src/main/java/com/vampire/loader/TestMetadata.java");

pub fn generate_android_manifest(
    permissions: &[String],
    services: &[crate::ManifestComponent],
    receivers: &[crate::ManifestComponent],
) -> String {
    let mut manifest = String::from(
        r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="com.vampire.host">

    <uses-sdk android:minSdkVersion="24" android:targetSdkVersion="30" />
"#,
    );

    for permission in permissions {
        manifest.push_str(&format!(
            "    <uses-permission android:name=\"{}\" />\n",
            permission
        ));
    }

    manifest.push_str(
        "\n    <application android:label=\"Vampire Host\" android:debuggable=\"true\">\n",
    );
    manifest.push_str("        <!-- Minimal instrumentation for running tests -->\n");
    manifest.push_str("        <uses-library android:name=\"android.test.runner\" />\n");

    // Add services
    for service in services {
        manifest.push_str(&format!(
            "        <service android:name=\"{}\"",
            service.name
        ));
        for (key, value) in &service.attributes {
            manifest.push_str(&format!(" android:{}=\"{}\"", key, value));
        }
        manifest.push_str(" />\n");
    }

    // Add receivers
    for receiver in receivers {
        manifest.push_str(&format!(
            "        <receiver android:name=\"{}\"",
            receiver.name
        ));
        for (key, value) in &receiver.attributes {
            manifest.push_str(&format!(" android:{}=\"{}\"", key, value));
        }
        manifest.push_str(" />\n");
    }

    manifest.push_str(
        r#"    </application>

    <instrumentation
        android:name=".VampireInstrumentation"
        android:targetPackage="com.vampire.host"
        android:label="Vampire Test Runner" />

</manifest>
"#,
    );

    manifest
}

pub const NETWORK_SECURITY_CONFIG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<network-security-config>
    <base-config cleartextTrafficPermitted="true">
        <trust-anchors>
            <certificates src="system" />
        </trust-anchors>
    </base-config>
</network-security-config>
"#;

fn generate_resource_xml(resources: &toml::Table) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n");

    for (name, value) in resources {
        match value {
            toml::Value::String(s) => {
                xml.push_str(&format!("    <string name=\"{}\">{}</string>\n", name, s));
            }
            toml::Value::Integer(i) => {
                xml.push_str(&format!("    <integer name=\"{}\">{}</integer>\n", name, i));
            }
            toml::Value::Boolean(b) => {
                xml.push_str(&format!("    <bool name=\"{}\">{}</bool>\n", name, b));
            }
            toml::Value::Array(arr) => {
                xml.push_str(&format!("    <string-array name=\"{}\">\n", name));
                for item in arr {
                    if let toml::Value::String(s) = item {
                        xml.push_str(&format!("        <item>{}</item>\n", s));
                    }
                }
                xml.push_str("    </string-array>\n");
            }
            _ => {
                eprintln!(
                    "Warning: Unsupported resource type for '{}', skipping",
                    name
                );
            }
        }
    }

    xml.push_str("</resources>\n");
    xml
}

pub fn write_host_files(
    build_dir: &Path,
    permissions: &[String],
    resources: &HashMap<String, toml::Table>,
    components: &crate::ManifestComponents,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create directory structure
    let manifest_path = build_dir.join("AndroidManifest.xml");
    let res_values_dir = build_dir.join("res/values");
    let res_xml_dir = build_dir.join("res/xml");
    let java_host_dir = build_dir.join("java/com/vampire/host");
    let java_loader_dir = build_dir.join("java/com/vampire/loader");

    std::fs::create_dir_all(&res_values_dir)?;
    std::fs::create_dir_all(&res_xml_dir)?;
    std::fs::create_dir_all(&java_host_dir)?;
    std::fs::create_dir_all(&java_loader_dir)?;

    // Write files
    let manifest_content =
        generate_android_manifest(permissions, &components.services, &components.receivers);
    std::fs::write(&manifest_path, manifest_content)?;
    std::fs::write(res_values_dir.join("strings.xml"), STRINGS_XML)?;
    std::fs::write(
        res_xml_dir.join("network_security_config.xml"),
        NETWORK_SECURITY_CONFIG,
    )?;
    std::fs::write(
        java_host_dir.join("VampireInstrumentation.java"),
        VAMPIRE_INSTRUMENTATION,
    )?;
    std::fs::write(java_loader_dir.join("TestRunner.java"), TEST_RUNNER)?;
    std::fs::write(java_loader_dir.join("TestMetadata.java"), TEST_METADATA)?;

    // Write custom resource files from TOML
    for (filename, resource_table) in resources {
        let xml_content = generate_resource_xml(resource_table);
        let file_path = res_values_dir.join(filename);
        std::fs::write(&file_path, xml_content)?;
        eprintln!("  Generated resource file: {}", file_path.display());
    }

    Ok(())
}
