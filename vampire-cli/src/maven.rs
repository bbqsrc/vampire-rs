use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MavenCoordinate {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
}

impl MavenCoordinate {
    pub fn parse(coord: &str) -> Result<Self, String> {
        let parts: Vec<&str> = coord.split(':').collect();
        if parts.len() != 3 {
            return Err(format!(
                "Invalid Maven coordinate '{}'. Expected format: groupId:artifactId:version",
                coord
            ));
        }

        Ok(Self {
            group_id: parts[0].to_string(),
            artifact_id: parts[1].to_string(),
            version: parts[2].to_string(),
        })
    }

    pub fn to_path(&self, extension: &str) -> String {
        format!(
            "{}/{}/{}/{}-{}.{}",
            self.group_id.replace('.', "/"),
            self.artifact_id,
            self.version,
            self.artifact_id,
            self.version,
            extension
        )
    }

    pub fn key(&self) -> String {
        format!("{}:{}", self.group_id, self.artifact_id)
    }

    pub fn metadata_path(&self) -> String {
        format!(
            "{}/{}/maven-metadata.xml",
            self.group_id.replace('.', "/"),
            self.artifact_id
        )
    }
}

impl std::fmt::Display for MavenCoordinate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.group_id, self.artifact_id, self.version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResolvedArtifact {
    pub coordinate: MavenCoordinate,
    pub jar_path: PathBuf,
    pub is_aar: bool,
    /// Native libraries extracted from AAR: (architecture, library_path)
    /// e.g., ("arm64-v8a", "path/to/libcronet.so")
    pub native_libs: Vec<(String, PathBuf)>,
    /// AndroidManifest.xml path if extracted from AAR
    pub manifest_path: Option<PathBuf>,
    /// Android resources directory if extracted from AAR
    pub res_dir: Option<PathBuf>,
    /// R.txt file path if extracted from AAR
    pub r_txt_path: Option<PathBuf>,
    /// Package name from AndroidManifest.xml if this is an AAR
    pub package_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VampireLock {
    pub version: String,
    pub artifacts: Vec<LockedArtifact>,
    pub metadata: LockMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedArtifact {
    pub requested: String,
    pub resolved: String,
    pub artifact_type: String,
    pub blake3: Option<String>,
    pub source_url: Option<String>,
    pub transitive: bool,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockMetadata {
    pub generated_at: String,
    pub repositories: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DependencyNode {
    pub coordinate: MavenCoordinate,
    pub depth: usize,
    pub is_transitive: bool,
    pub download_urls: Vec<String>,
    pub parent: Option<MavenCoordinate>,
    pub children: Vec<MavenCoordinate>,
}

pub struct MavenResolver {
    client: frakt::Client,
    cache_dir: PathBuf,
    repositories: Vec<String>,
    lock_file_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    fn parse(version_str: &str) -> Option<Self> {
        let parts: Vec<&str> = version_str.split('.').collect();
        if parts.len() != 3 {
            return None;
        }

        Some(Version {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
        })
    }

    fn is_compatible_with(&self, requested: &Version) -> bool {
        // Same major version, and >= minor.patch
        self.major == requested.major &&
        (self.minor > requested.minor ||
         (self.minor == requested.minor && self.patch >= requested.patch))
    }
}

impl MavenResolver {
    pub fn new(cache_dir: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let client = frakt::Client::builder()
            .backend(frakt::BackendType::Reqwest)
            .user_agent("vampire-cli/0.1.0")
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        let repositories = vec![
            "https://dl.google.com/dl/android/maven2".to_string(),
            "https://repo.maven.apache.org/maven2".to_string(),
        ];

        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create Maven cache directory {}: {}", cache_dir.display(), e))?;

        Ok(Self {
            client,
            cache_dir,
            repositories,
            lock_file_path: None,
        })
    }

    pub fn with_lock_file(mut self, lock_file_path: PathBuf) -> Self {
        self.lock_file_path = Some(lock_file_path);
        self
    }

    pub fn read_lock(&self) -> Result<Option<VampireLock>, Box<dyn std::error::Error>> {
        let Some(ref lock_path) = self.lock_file_path else {
            return Ok(None);
        };

        if !lock_path.exists() {
            return Ok(None);
        }

        let contents = std::fs::read_to_string(lock_path)
            .map_err(|e| format!("Failed to read lock file {}: {}", lock_path.display(), e))?;

        let lock: VampireLock = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse lock file {}: {}", lock_path.display(), e))?;

        Ok(Some(lock))
    }

    pub fn write_lock(&self, lock: &VampireLock) -> Result<(), Box<dyn std::error::Error>> {
        let Some(ref lock_path) = self.lock_file_path else {
            return Ok(());
        };

        let contents = toml::to_string_pretty(lock)
            .map_err(|e| format!("Failed to serialize lock file: {}", e))?;

        std::fs::write(lock_path, contents)
            .map_err(|e| format!("Failed to write lock file {}: {}", lock_path.display(), e))?;

        eprintln!("📝 Lock file written: {}", lock_path.display());
        Ok(())
    }

    pub fn validate_lock(&self, lock: &VampireLock, requested_coords: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
        let requested_set: HashSet<String> = requested_coords
            .iter()
            .map(|c| {
                let coord = MavenCoordinate::parse(c)?;
                Ok(coord.key())
            })
            .collect::<Result<_, Box<dyn std::error::Error>>>()?;

        let lock_direct_deps: HashSet<String> = lock
            .artifacts
            .iter()
            .filter(|a| !a.transitive)
            .map(|a| {
                let coord = MavenCoordinate::parse(&a.requested)?;
                Ok(coord.key())
            })
            .collect::<Result<_, Box<dyn std::error::Error>>>()?;

        Ok(requested_set == lock_direct_deps)
    }

    pub async fn resolve(
        &self,
        coordinates: &[String],
    ) -> Result<Vec<ResolvedArtifact>, Box<dyn std::error::Error>> {
        self.resolve_with_lock(coordinates, false).await
    }

    pub async fn resolve_with_lock(
        &self,
        coordinates: &[String],
        force_update: bool,
    ) -> Result<Vec<ResolvedArtifact>, Box<dyn std::error::Error>> {
        // Try to use lock file if available and valid
        if !force_update {
            if let Some(lock) = self.read_lock()? {
                if self.validate_lock(&lock, coordinates)? {
                    eprintln!("📦 Using lock file (vampire.lock)");
                    return self.resolve_from_lock(&lock).await;
                } else {
                    eprintln!("⚠️  Lock file is stale, re-resolving dependencies");
                }
            }
        }

        // Full resolution
        eprintln!("🔍 Resolving dependencies...");
        let mut resolved = HashMap::new();
        let mut visited = HashSet::new();
        let mut lock_artifacts = Vec::new();

        for coord_str in coordinates {
            let coord = MavenCoordinate::parse(coord_str)?;
            self.resolve_recursive(&coord, 0, &mut resolved, &mut visited, &mut lock_artifacts, coord_str)
                .await?;
        }

        let mut artifacts = Vec::new();
        for (_, artifact) in resolved {
            artifacts.push(artifact);
        }

        // Write lock file
        let lock = VampireLock {
            version: "1".to_string(),
            artifacts: lock_artifacts,
            metadata: LockMetadata {
                generated_at: chrono::Utc::now().to_rfc3339(),
                repositories: self.repositories.clone(),
            },
        };
        self.write_lock(&lock)?;

        Ok(artifacts)
    }

    async fn resolve_from_lock(
        &self,
        lock: &VampireLock,
    ) -> Result<Vec<ResolvedArtifact>, Box<dyn std::error::Error>> {
        let mut artifacts = Vec::new();

        for locked in &lock.artifacts {
            let coord = MavenCoordinate::parse(&locked.resolved)?;
            let (artifact, _source_url) = self.download_artifact(&coord).await?;

            // Verify BLAKE3 checksum if available
            if let Some(ref expected_hash) = locked.blake3 {
                let artifact_type = &locked.artifact_type;
                let artifact_dir = self.cache_dir.join(&coord.group_id).join(&coord.artifact_id).join(&coord.version);
                let artifact_file = artifact_dir.join(format!("{}-{}.{}", coord.artifact_id, coord.version, artifact_type));
                let actual_hash = Self::calculate_blake3(&artifact_file)?;
                if &actual_hash != expected_hash {
                    return Err(format!(
                        "BLAKE3 checksum mismatch for {}: expected {}, got {}",
                        locked.resolved, expected_hash, actual_hash
                    ).into());
                }
            }

            artifacts.push(artifact);
        }

        Ok(artifacts)
    }

    pub async fn resolve_dependencies_dry_run(
        &self,
        coordinates: &[String],
    ) -> Result<Vec<DependencyNode>, Box<dyn std::error::Error>> {
        let mut resolved = HashMap::new();
        let mut visited = HashSet::new();

        for coord_str in coordinates {
            let coord = MavenCoordinate::parse(coord_str)?;
            self.resolve_dry_run_recursive(&coord, 0, &mut resolved, &mut visited, None)
                .await?;
        }

        let mut nodes = Vec::new();
        for (_, node) in resolved {
            nodes.push(node);
        }

        // Sort by depth then by coordinate
        nodes.sort_by(|a, b| {
            a.depth.cmp(&b.depth).then_with(|| {
                a.coordinate
                    .group_id
                    .cmp(&b.coordinate.group_id)
                    .then_with(|| a.coordinate.artifact_id.cmp(&b.coordinate.artifact_id))
            })
        });

        Ok(nodes)
    }

    fn resolve_dry_run_recursive<'a>(
        &'a self,
        coord: &'a MavenCoordinate,
        depth: usize,
        resolved: &'a mut HashMap<String, DependencyNode>,
        visited: &'a mut HashSet<String>,
        parent: Option<MavenCoordinate>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error>>> + 'a>> {
        Box::pin(async move {
            let key = coord.key();

            if visited.contains(&key) {
                // Even if visited, add this as a child to the parent
                if let Some(ref parent_coord) = parent {
                    let parent_key = parent_coord.key();
                    if let Some(parent_node) = resolved.get_mut(&parent_key) {
                        if !parent_node.children.contains(coord) {
                            parent_node.children.push(coord.clone());
                        }
                    }
                }
                return Ok(());
            }
            visited.insert(key.clone());

            // Apply nearest-wins: if already resolved at shallower depth, skip
            if let Some(existing) = resolved.get(&key) {
                if depth >= existing.depth {
                    return Ok(());
                }
            }

            // Generate download URLs for all repositories
            let mut download_urls = Vec::new();
            for repo in &self.repositories {
                // Try AAR first, then JAR
                for extension in ["aar", "jar"] {
                    let path = coord.to_path(extension);
                    download_urls.push(format!("{}/{}", repo, path));
                }
            }

            let node = DependencyNode {
                coordinate: coord.clone(),
                depth,
                is_transitive: depth > 0,
                download_urls,
                parent: parent.clone(),
                children: Vec::new(),
            };
            resolved.insert(key.clone(), node);

            // Add this as a child to the parent
            if let Some(ref parent_coord) = parent {
                let parent_key = parent_coord.key();
                if let Some(parent_node) = resolved.get_mut(&parent_key) {
                    if !parent_node.children.contains(coord) {
                        parent_node.children.push(coord.clone());
                    }
                }
            }

            // Download and parse POM to get transitive dependencies
            let pom = self.download_pom(coord).await?;
            let dependencies = self.parse_pom_dependencies(&pom, coord)?;

            for dep_coord in dependencies {
                // Upgrade to latest compatible version
                let upgraded_version = self.find_latest_compatible_version(&dep_coord).await?;
                let upgraded_coord = MavenCoordinate {
                    group_id: dep_coord.group_id.clone(),
                    artifact_id: dep_coord.artifact_id.clone(),
                    version: upgraded_version,
                };

                self.resolve_dry_run_recursive(&upgraded_coord, depth + 1, resolved, visited, Some(coord.clone()))
                    .await?;
            }

            Ok(())
        })
    }

    fn resolve_recursive<'a>(
        &'a self,
        coord: &'a MavenCoordinate,
        depth: usize,
        resolved: &'a mut HashMap<String, ResolvedArtifact>,
        visited: &'a mut HashSet<String>,
        lock_artifacts: &'a mut Vec<LockedArtifact>,
        requested_version: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error>>> + 'a>> {
        Box::pin(async move {
            let key = coord.key();

            if visited.contains(&key) {
                return Ok(());
            }
            visited.insert(key.clone());

            if resolved.contains_key(&key) {
                return Ok(());
            }

            let (artifact, source_url) = self.download_artifact(coord).await?;

            // Calculate BLAKE3 hash of the original AAR/JAR file
            let artifact_type = if artifact.is_aar { "aar" } else { "jar" };
            let artifact_dir = self.cache_dir.join(&coord.group_id).join(&coord.artifact_id).join(&coord.version);
            let artifact_file = artifact_dir.join(format!("{}-{}.{}", coord.artifact_id, coord.version, artifact_type));
            let blake3_hash = Self::calculate_blake3(&artifact_file).ok();

            lock_artifacts.push(LockedArtifact {
                requested: requested_version.to_string(),
                resolved: coord.to_string(),
                artifact_type: artifact_type.to_string(),
                blake3: blake3_hash,
                source_url,
                transitive: depth > 0,
                parent: None,
            });

            resolved.insert(key, artifact);

            let pom = self.download_pom(coord).await?;
            let dependencies = self.parse_pom_dependencies(&pom, coord)?;

            for dep_coord in dependencies {
                // Upgrade to latest compatible version
                let upgraded_version = self.find_latest_compatible_version(&dep_coord).await?;
                let upgraded_coord = MavenCoordinate {
                    group_id: dep_coord.group_id.clone(),
                    artifact_id: dep_coord.artifact_id.clone(),
                    version: upgraded_version,
                };

                let dep_requested = format!("{}:{}:{}", dep_coord.group_id, dep_coord.artifact_id, dep_coord.version);
                self.resolve_recursive(&upgraded_coord, depth + 1, resolved, visited, lock_artifacts, &dep_requested)
                    .await?;
            }

            Ok(())
        })
    }

    async fn download_artifact(
        &self,
        coord: &MavenCoordinate,
    ) -> Result<(ResolvedArtifact, Option<String>), Box<dyn std::error::Error>> {
        let artifact_dir = self.cache_dir.join(&coord.group_id).join(&coord.artifact_id).join(&coord.version);
        std::fs::create_dir_all(&artifact_dir)
            .map_err(|e| format!("Failed to create artifact directory {}: {}", artifact_dir.display(), e))?;

        // Try AAR first
        let aar_result = self.try_download(coord, "aar", &artifact_dir).await?;
        let aar_path = artifact_dir.join(format!("{}-{}.aar", coord.artifact_id, coord.version));

        let (extension, is_aar, source_url) = if aar_result.is_some() || aar_path.exists() {
            // AAR was downloaded or cached
            ("aar", true, aar_result)
        } else {
            // No AAR, try JAR
            let jar_result = self.try_download(coord, "jar", &artifact_dir).await?;
            let jar_path = artifact_dir.join(format!("{}-{}.jar", coord.artifact_id, coord.version));

            if jar_result.is_some() || jar_path.exists() {
                ("jar", false, jar_result)
            } else {
                return Err(format!(
                    "Could not download {}:{}:{} - no AAR or JAR found in any repository",
                    coord.group_id, coord.artifact_id, coord.version
                ).into());
            }
        };

        let artifact_path = artifact_dir.join(format!("{}-{}.{}", coord.artifact_id, coord.version, extension));

        // Verify the downloaded artifact is valid (both JAR and AAR are ZIP files)
        let test_file = std::fs::File::open(&artifact_path)
            .map_err(|e| format!("Failed to open downloaded artifact {}: {}", artifact_path.display(), e))?;

        zip::ZipArchive::new(test_file)
            .map_err(|e| format!(
                "Downloaded artifact {}:{}:{} is corrupt or invalid ({}). Try deleting {} and rebuilding.",
                coord.group_id, coord.artifact_id, coord.version, e, artifact_path.display()
            ))?;

        let (jar_path, native_libs, manifest_path, res_dir, r_txt_path, package_name) = if is_aar {
            self.extract_aar_contents(&artifact_path, &artifact_dir)?
        } else {
            (artifact_path, Vec::new(), None, None, None, None)
        };

        Ok((ResolvedArtifact {
            coordinate: coord.clone(),
            jar_path,
            is_aar,
            native_libs,
            manifest_path,
            res_dir,
            r_txt_path,
            package_name,
        }, source_url))
    }

    fn calculate_blake3(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = blake3::Hasher::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(hasher.finalize().to_hex().to_string())
    }

    async fn try_download(
        &self,
        coord: &MavenCoordinate,
        extension: &str,
        output_dir: &Path,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let output_file = output_dir.join(format!("{}-{}.{}", coord.artifact_id, coord.version, extension));

        if output_file.exists() {
            // Verify existing file is valid (non-empty and valid ZIP for jar/aar)
            if let Ok(metadata) = std::fs::metadata(&output_file) {
                if metadata.len() > 0 {
                    // Try to open as ZIP to verify it's valid
                    if let Ok(file) = std::fs::File::open(&output_file) {
                        if zip::ZipArchive::new(file).is_ok() {
                            eprintln!("Using cached {}: {}", extension, output_file.display());
                            // Return None because we don't know which URL it came from originally
                            return Ok(None);
                        }
                    }
                }
            }

            // File exists but is corrupt/empty, delete it
            eprintln!("Cached {} is corrupt, re-downloading: {}", extension, output_file.display());
            let _ = std::fs::remove_file(&output_file);
        }

        let path = coord.to_path(extension);

        for repo in &self.repositories {
            let url = format!("{}/{}", repo, path);
            eprintln!("Trying {}: {}", extension, url);

            let coord2 = coord.clone(); // For closure capture
            let download_builder = match self.client.download(url.as_str(), output_file.as_path()) {
                Ok(builder) => builder,
                Err(e) => {
                    let err_str = e.to_string();
                    eprintln!("Failed to create download for {}: {}", url, err_str);
                    if err_str.contains("404") || err_str.contains("Not Found") {
                        continue;
                    } else {
                        return Err(Box::new(e));
                    }
                }
            };

            let res = download_builder
                .progress(move |downloaded, total| {
                    if let Some(total) = total {
                        let percent = (downloaded as f64 / total as f64) * 100.0;
                        print!("Downloading archive {}: {:.2}%\r", coord2.key(), percent);
                    } else {
                        print!("Downloading archive {}: {} bytes\r", coord2.key(), downloaded);
                    }
                })
                .send()
                .await;

            match res {
                Ok(_) => {
                    println!("Downloaded {}:{}:{} to {}", coord.group_id, coord.artifact_id, coord.version, output_file.display());
                    return Ok(Some(url));
                }
                Err(e) => {
                    let err_str = e.to_string();
                    eprintln!("Failed to download from {}: {}", url, err_str);

                    // Clean up any corrupt file that might have been created
                    if output_file.exists() {
                        let _ = std::fs::remove_file(&output_file);
                    }

                    if err_str.contains("404") || err_str.contains("Not Found") {
                        // 404 means this extension doesn't exist in this repo, try next repo
                        continue;
                    } else {
                        // Other errors (network, etc) should propagate
                        return Err(Box::new(e));
                    }
                }
            }
        }

        Ok(None)
    }

    async fn download_pom(
        &self,
        coord: &MavenCoordinate,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let pom_dir = self.cache_dir.join(&coord.group_id).join(&coord.artifact_id).join(&coord.version);
        std::fs::create_dir_all(&pom_dir)
            .map_err(|e| format!("Failed to create POM directory {}: {}", pom_dir.display(), e))?;

        let pom_file = pom_dir.join(format!("{}-{}.pom", coord.artifact_id, coord.version));

        if pom_file.exists() {
            return std::fs::read_to_string(&pom_file)
                .map_err(|e| format!("Failed to read cached POM file {}: {}", pom_file.display(), e).into());
        }

        let path = coord.to_path("pom");

        for repo in &self.repositories {
            let url = format!("{}/{}", repo, path);
            eprintln!("Trying POM: {}", url);

            let coord2 = coord.clone(); // For closure capture
            let download_builder = match self.client.download(url.as_str(), pom_file.as_path()) {
                Ok(builder) => builder,
                Err(e) => {
                    let err_str = e.to_string();
                    eprintln!("Failed to create download for {}: {}", url, err_str);
                    if err_str.contains("404") || err_str.contains("Not Found") {
                        continue;
                    } else {
                        return Err(Box::new(e));
                    }
                }
            };

            let res = download_builder
                .progress(move |downloaded, total| {
                    if let Some(total) = total {
                        let percent = (downloaded as f64 / total as f64) * 100.0;
                        println!("Downloading POM {}: {:.2}%", coord2.key(), percent);
                    } else {
                        println!("Downloading POM {}: {} bytes", coord2.key(), downloaded);
                    }
                })
                .send()
                .await;

            match res {
                Ok(_) => {
                    println!("Downloaded POM for {}:{}:{}", coord.group_id, coord.artifact_id, coord.version);
                    return std::fs::read_to_string(&pom_file)
                        .map_err(|e| format!("Failed to read downloaded POM file {}: {}", pom_file.display(), e).into());
                }
                Err(e) => {
                    let err_str = e.to_string();
                    eprintln!("Failed to download from {}: {}", url, err_str);
                    if err_str.contains("404") || err_str.contains("Not Found") {
                        continue;
                    } else {
                        return Err(Box::new(e));
                    }
                }
            }
        }

        Err(format!("Could not download POM for {}:{}:{} from any repository", coord.group_id, coord.artifact_id, coord.version).into())
    }

    async fn download_maven_metadata(
        &self,
        coord: &MavenCoordinate,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let metadata_path = coord.metadata_path();

        for repo in &self.repositories {
            let url = format!("{}/{}", repo, metadata_path);

            match self.client.get(url.as_str()) {
                Ok(builder) => {
                    match builder.send().await {
                        Ok(response) => {
                            if let Ok(body) = response.text().await {
                                return Ok(body);
                            }
                        }
                        Err(_) => continue,
                    }
                }
                Err(_) => continue,
            }
        }

        Err(format!("Could not download maven-metadata.xml for {}:{} from any repository", coord.group_id, coord.artifact_id).into())
    }

    fn parse_versions_from_metadata(&self, metadata_xml: &str) -> Vec<String> {
        let mut versions = Vec::new();

        if let Ok(doc) = xmlem::Document::from_reader(std::io::Cursor::new(metadata_xml)) {
            let root = doc.root();

            // Find versioning/versions element
            for child in root.children(&doc) {
                if child.name(&doc) == "versioning" {
                    for versioning_child in child.children(&doc) {
                        if versioning_child.name(&doc) == "versions" {
                            for version_elem in versioning_child.children(&doc) {
                                if version_elem.name(&doc) == "version" {
                                    if let Some(text_node) = version_elem.child_nodes(&doc).first() {
                                        if let xmlem::key::Node::Text(t) = text_node {
                                            versions.push(t.as_str(&doc).to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        versions
    }

    async fn find_latest_compatible_version(
        &self,
        coord: &MavenCoordinate,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let requested_version = Version::parse(&coord.version);

        // If can't parse as semver, just use the requested version
        let Some(requested) = requested_version else {
            return Ok(coord.version.clone());
        };

        // Download maven-metadata.xml
        let metadata = match self.download_maven_metadata(coord).await {
            Ok(m) => m,
            Err(_) => {
                // If metadata not available, use requested version
                return Ok(coord.version.clone());
            }
        };

        // Parse all available versions
        let available_versions = self.parse_versions_from_metadata(&metadata);

        // Find highest compatible version
        let mut best_version = coord.version.clone();
        let mut best_parsed = requested.clone();

        for version_str in available_versions {
            if let Some(parsed) = Version::parse(&version_str) {
                if parsed.is_compatible_with(&requested) && parsed > best_parsed {
                    best_version = version_str;
                    best_parsed = parsed;
                }
            }
        }

        if best_version != coord.version {
            eprintln!("⬆️  Upgrading {} from {} to {}", coord.key(), coord.version, best_version);
        }

        Ok(best_version)
    }

    fn normalize_version(version: &str) -> String {
        // Handle Maven version ranges: [1.0] means exactly 1.0, [1.0,2.0) means 1.0 <= version < 2.0
        // For simplicity, we'll extract the first version number from ranges
        let trimmed = version.trim();

        if trimmed.starts_with('[') || trimmed.starts_with('(') {
            // Extract version from range syntax like [2.5.1] or [1.0,2.0)
            let inner = trimmed.trim_start_matches('[').trim_start_matches('(');
            let version_part = inner.split(',').next().unwrap_or(inner);
            version_part.trim_end_matches(']').trim_end_matches(')').trim().to_string()
        } else {
            trimmed.to_string()
        }
    }

    fn resolve_property(value: &str, current_coord: &MavenCoordinate) -> String {
        // Resolve Maven property placeholders like ${project.groupId} and ${project.version}
        value
            .replace("${project.groupId}", &current_coord.group_id)
            .replace("${project/groupId}", &current_coord.group_id)
            .replace("${pom.groupId}", &current_coord.group_id)
            .replace("${project.version}", &current_coord.version)
            .replace("${project/version}", &current_coord.version)
            .replace("${pom.version}", &current_coord.version)
            .replace("${project.artifactId}", &current_coord.artifact_id)
            .replace("${project/artifactId}", &current_coord.artifact_id)
            .replace("${pom.artifactId}", &current_coord.artifact_id)
    }

    fn parse_pom_dependencies(
        &self,
        pom_xml: &str,
        current_coord: &MavenCoordinate,
    ) -> Result<Vec<MavenCoordinate>, Box<dyn std::error::Error>> {
        let mut dependencies = Vec::new();

        let doc = xmlem::Document::from_reader(std::io::Cursor::new(pom_xml))?;
        let root = doc.root();

        // Find all children of root and look for dependencies element
        for child in root.children(&doc) {
            if child.name(&doc) == "dependencies" {
                // Iterate through dependency elements
                for dep in child.children(&doc) {
                    if dep.name(&doc) != "dependency" {
                        continue;
                    }

                    let mut group_id = None;
                    let mut artifact_id = None;
                    let mut version = None;
                    let mut scope = "compile";

                    // Parse dependency fields
                    for field in dep.children(&doc) {
                        match field.name(&doc) {
                            "groupId" => {
                                if let Some(text_node) = field.child_nodes(&doc).first() {
                                    if let xmlem::key::Node::Text(t) = text_node {
                                        group_id = Some(Self::resolve_property(t.as_str(&doc), current_coord));
                                    }
                                }
                            }
                            "artifactId" => {
                                if let Some(text_node) = field.child_nodes(&doc).first() {
                                    if let xmlem::key::Node::Text(t) = text_node {
                                        artifact_id = Some(Self::resolve_property(t.as_str(&doc), current_coord));
                                    }
                                }
                            }
                            "version" => {
                                if let Some(text_node) = field.child_nodes(&doc).first() {
                                    if let xmlem::key::Node::Text(t) = text_node {
                                        let resolved = Self::resolve_property(t.as_str(&doc), current_coord);
                                        version = Some(Self::normalize_version(&resolved));
                                    }
                                }
                            }
                            "scope" => {
                                if let Some(text_node) = field.child_nodes(&doc).first() {
                                    if let xmlem::key::Node::Text(t) = text_node {
                                        scope = t.as_str(&doc);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    // Only include compile and runtime scopes
                    // Skip: test, provided, system, import
                    if scope != "compile" && scope != "runtime" {
                        continue;
                    }

                    if let (Some(group), Some(artifact), Some(ver)) = (group_id, artifact_id, version) {
                        dependencies.push(MavenCoordinate {
                            group_id: group,
                            artifact_id: artifact,
                            version: ver,
                        });
                    }
                }
            }
        }

        Ok(dependencies)
    }

    fn extract_aar_contents(
        &self,
        aar_path: &Path,
        output_dir: &Path,
    ) -> Result<(PathBuf, Vec<(String, PathBuf)>, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>, Option<String>), Box<dyn std::error::Error>> {
        let file = std::fs::File::open(aar_path)
            .map_err(|e| format!("Failed to open AAR file {}: {}", aar_path.display(), e))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("Failed to read AAR as ZIP archive {}: {}", aar_path.display(), e))?;

        let classes_jar_path = output_dir.join("classes.jar");
        let mut native_libs = Vec::new();
        let mut manifest_path = None;
        let mut res_dir: Option<PathBuf> = None;
        let mut r_txt_path: Option<PathBuf> = None;
        let mut package_name: Option<String> = None;
        let mut found_classes_jar = false;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)
                .map_err(|e| format!("Failed to read entry {} from AAR {}: {}", i, aar_path.display(), e))?;
            let name = file.name().to_string();

            if name == "classes.jar" {
                let mut outfile = std::fs::File::create(&classes_jar_path)
                    .map_err(|e| format!("Failed to create classes.jar at {}: {}", classes_jar_path.display(), e))?;
                std::io::copy(&mut file, &mut outfile)
                    .map_err(|e| format!("Failed to extract classes.jar from {} to {}: {}", aar_path.display(), classes_jar_path.display(), e))?;
                found_classes_jar = true;
            } else if name == "AndroidManifest.xml" {
                let manifest_file_path = output_dir.join("AndroidManifest.xml");

                // Read manifest contents to print
                let mut manifest_contents = Vec::new();
                std::io::copy(&mut file, &mut manifest_contents)
                    .map_err(|e| format!("Failed to read AndroidManifest.xml from {}: {}", aar_path.display(), e))?;

                // Write to file
                std::fs::write(&manifest_file_path, &manifest_contents)
                    .map_err(|e| format!("Failed to write AndroidManifest.xml to {}: {}", manifest_file_path.display(), e))?;

                // Parse package name from manifest
                if let Ok(manifest_str) = String::from_utf8(manifest_contents.clone()) {
                    if let Ok(doc) = xmlem::Document::from_reader(std::io::Cursor::new(&manifest_str)) {
                        let root = doc.root();
                        if let Some(pkg) = root.attribute(&doc, "package") {
                            package_name = Some(pkg.to_string());
                            eprintln!("Found package name in AAR manifest: {}", pkg);
                        }
                    }

                    // Print the manifest contents
                    eprintln!("\n=== AndroidManifest.xml from {} ===", aar_path.display());
                    eprintln!("{}", manifest_str);
                    eprintln!("=== End manifest ===\n");
                } else {
                    eprintln!("(Binary manifest - cannot display as text)");
                }

                manifest_path = Some(manifest_file_path);
            } else if name == "R.txt" {
                // Extract R.txt
                let r_txt_file_path = output_dir.join("R.txt");
                let mut outfile = std::fs::File::create(&r_txt_file_path)
                    .map_err(|e| format!("Failed to create R.txt at {}: {}", r_txt_file_path.display(), e))?;
                std::io::copy(&mut file, &mut outfile)
                    .map_err(|e| format!("Failed to extract R.txt from {}: {}", aar_path.display(), e))?;
                r_txt_path = Some(r_txt_file_path);
                eprintln!("Extracted R.txt from AAR: {}", aar_path.display());
            } else if name.starts_with("res/") && !name.ends_with('/') {
                // Extract resource files
                let res_output_dir = output_dir.join("res");
                if res_dir.is_none() {
                    std::fs::create_dir_all(&res_output_dir)
                        .map_err(|e| format!("Failed to create res directory {}: {}", res_output_dir.display(), e))?;
                    res_dir = Some(res_output_dir.clone());
                }

                // Get relative path within res/
                let res_file_path = output_dir.join(&name);
                if let Some(parent) = res_file_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create resource subdirectory {}: {}", parent.display(), e))?;
                }

                let mut outfile = std::fs::File::create(&res_file_path)
                    .map_err(|e| format!("Failed to create resource file at {}: {}", res_file_path.display(), e))?;
                std::io::copy(&mut file, &mut outfile)
                    .map_err(|e| format!("Failed to extract {} from {}: {}", name, aar_path.display(), e))?;
            } else if name.starts_with("jni/") && name.ends_with(".so") {
                // Extract native library: jni/<arch>/libname.so
                let parts: Vec<&str> = name.split('/').collect();
                if parts.len() == 3 {
                    let arch = parts[1];
                    let lib_name = parts[2];

                    // Create arch-specific output directory
                    let arch_dir = output_dir.join("jni").join(arch);
                    std::fs::create_dir_all(&arch_dir)
                        .map_err(|e| format!("Failed to create jni/{} directory {}: {}", arch, arch_dir.display(), e))?;

                    let lib_path = arch_dir.join(lib_name);
                    let mut outfile = std::fs::File::create(&lib_path)
                        .map_err(|e| format!("Failed to create native library at {}: {}", lib_path.display(), e))?;
                    std::io::copy(&mut file, &mut outfile)
                        .map_err(|e| format!("Failed to extract {} from {}: {}", name, aar_path.display(), e))?;

                    eprintln!("Extracted native library: {} -> {}", name, lib_path.display());
                    native_libs.push((arch.to_string(), lib_path));
                }
            }
        }

        // If no classes.jar found, create an empty one (some AARs are metadata-only)
        if !found_classes_jar {
            eprintln!("No classes.jar in AAR {}, creating empty JAR", aar_path.display());

            // Create empty JAR file
            let empty_jar = std::fs::File::create(&classes_jar_path)
                .map_err(|e| format!("Failed to create empty classes.jar at {}: {}", classes_jar_path.display(), e))?;

            let mut zip_writer = zip::ZipWriter::new(empty_jar);

            // Add a minimal manifest to make it a valid JAR
            let options = zip::write::FileOptions::<()>::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip_writer.start_file("META-INF/MANIFEST.MF", options)
                .map_err(|e| format!("Failed to create manifest in empty JAR: {}", e))?;
            zip_writer.write_all(b"Manifest-Version: 1.0\n")
                .map_err(|e| format!("Failed to write manifest: {}", e))?;

            zip_writer.finish()
                .map_err(|e| format!("Failed to finalize empty JAR: {}", e))?;
        }

        Ok((classes_jar_path, native_libs, manifest_path, res_dir, r_txt_path, package_name))
    }

    pub fn print_dependency_tree(&self, nodes: &[DependencyNode]) {
        eprintln!("\nDependency Tree:");

        // Find root nodes (depth 0, no parent)
        let roots: Vec<_> = nodes.iter()
            .filter(|n| n.depth == 0)
            .collect();

        // Build a map for quick lookup
        let node_map: HashMap<String, &DependencyNode> = nodes.iter()
            .map(|n| (n.coordinate.key(), n))
            .collect();

        for root in roots {
            self.print_node_recursive(root, &node_map, "", true);
        }
    }

    fn print_node_recursive(&self, node: &DependencyNode, node_map: &HashMap<String, &DependencyNode>, prefix: &str, is_last: bool) {
        let connector = if is_last { "└── " } else { "├── " };
        eprintln!("{}{}{}", prefix, connector, node.coordinate);

        let child_prefix = if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };

        for (i, child_coord) in node.children.iter().enumerate() {
            let is_last_child = i == node.children.len() - 1;
            if let Some(child_node) = node_map.get(&child_coord.key()) {
                self.print_node_recursive(child_node, node_map, &child_prefix, is_last_child);
            }
        }
    }

    pub fn detect_conflicts(&self, _nodes: &[DependencyNode]) {
        //
    }
}
