//! Signed update metadata, bounded downloads and rollback-safe directory replacement.
//! This module never runs project code or modifies the user's settings.
use std::{collections::BTreeMap, fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Component, Path, PathBuf}, process::{Command, Stdio}};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const IDE_FEED: &str = "https://github.com/sherhn/Light/releases/latest/download/updates.json";
pub const MODULE_FEED: &str = "https://github.com/sherhn/Light-Modules/releases/latest/download/updates.json";
pub const MAX_ARCHIVE: u64 = 600 * 1024 * 1024;
const MAX_EXPANDED: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope { pub signed: String, pub signature: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub kind: String,
    pub version: String,
    pub issued: u64,
    pub expires: u64,
    pub targets: BTreeMap<String, Artifact>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact { pub name: String, pub url: String, pub size: u64, pub sha256: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inventory { pub schema: u32, pub version: String, pub entrypoint: String, pub files: BTreeMap<String, String> }

pub fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
fn unhex<const N: usize>(text: &str) -> Result<[u8; N], String> {
    let text = text.trim();
    if text.len() != N * 2 || !text.bytes().all(|b| b.is_ascii_hexdigit()) { return Err("invalid update signature/key/hash".into()); }
    let mut bytes = [0; N];
    for (i, byte) in bytes.iter_mut().enumerate() { *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?; }
    Ok(bytes)
}
pub fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() }
pub fn version(text: &str) -> Result<[u64; 3], String> {
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || p.len() > 10 || !p.bytes().all(|b| b.is_ascii_digit())) { return Err("automatic updates require a stable semantic version".into()); }
    Ok([parts[0].parse().map_err(|_| "invalid version")?, parts[1].parse().map_err(|_| "invalid version")?, parts[2].parse().map_err(|_| "invalid version")?])
}
fn plain(text: &str) -> bool { !text.is_empty() && text != "." && text != ".." && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) }
pub fn verify(bytes: &[u8], public_key: &str, expected_kind: &str, clock: u64) -> Result<Manifest, String> {
    if bytes.len() > 1024 * 1024 { return Err("update metadata is too large".into()); }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let key = VerifyingKey::from_bytes(&unhex(public_key)?).map_err(|e| e.to_string())?;
    key.verify_strict(envelope.signed.as_bytes(), &Signature::from_bytes(&unhex(&envelope.signature)?)).map_err(|_| "update signature verification failed")?;
    let manifest: Manifest = serde_json::from_str(&envelope.signed).map_err(|e| e.to_string())?;
    version(&manifest.version)?;
    if manifest.schema != 1 || manifest.kind != expected_kind || !matches!(expected_kind, "ide" | "modules") || manifest.issued > clock.saturating_add(300) || manifest.expires <= clock || manifest.expires <= manifest.issued {
        return Err("incompatible, expired or future-dated update metadata".into());
    }
    let repo = if expected_kind == "ide" { "Light" } else { "Light-Modules" };
    let prefix = format!("https://github.com/sherhn/{repo}/releases/download/v{}/", manifest.version);
    if manifest.targets.is_empty() { return Err("update has no targets".into()); }
    for artifact in manifest.targets.values() {
        unhex::<32>(&artifact.sha256)?;
        if !plain(&artifact.name) || artifact.url != format!("{prefix}{}", artifact.name) || artifact.size == 0 || artifact.size > MAX_ARCHIVE { return Err("unsafe or oversized update target".into()); }
    }
    Ok(manifest)
}

pub fn platform() -> String {
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH { "x86_64" => "x64", "aarch64" => "arm64", other => other };
    format!("{os}-{arch}")
}
pub fn file_hash(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new(); let mut buffer = [0; 65536];
    loop { let n = file.read(&mut buffer).map_err(|e| e.to_string())?; if n == 0 { break } hash.update(&buffer[..n]); }
    Ok(hex(&hash.finalize()))
}
/// Hash native code independently of the embedded Apple signature, which seals this inventory itself.
/// The complete final signature is still authenticated by the signed release archive's SHA-256.
pub fn macho_hash(path: &Path) -> Result<String, String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_ARCHIVE { return Err("Mach-O executable is too large".into()); }
    let mut bytes = fs::read(path).map_err(|e| e.to_string())?;
    let number = |data: &[u8], offset: usize| -> Result<usize, String> {
        let word: [u8;4] = data.get(offset..offset+4).ok_or("truncated Mach-O")?.try_into().unwrap();
        Ok(u32::from_le_bytes(word) as usize)
    };
    if bytes.len() < 32 || number(&bytes,0)? != 0xfeedfacf { return Err("native macOS updates require a thin 64-bit Mach-O".into()); }
    let count = number(&bytes,16)?; let end = 32_usize.checked_add(number(&bytes,20)?).ok_or("invalid Mach-O length")?;
    if end > bytes.len() || count > 10000 { return Err("invalid Mach-O commands".into()); }
    let mut cursor = 32; let mut signature = None;
    for _ in 0..count {
        if cursor + 8 > end { return Err("truncated Mach-O command".into()); }
        let command = number(&bytes,cursor)?; let size = number(&bytes,cursor+4)?;
        let next = cursor.checked_add(size).ok_or("invalid Mach-O command size")?;
        if size < 8 || next > end { return Err("invalid Mach-O command size".into()); }
        if command == 0x1d {
            if size < 16 || signature.is_some() { return Err("invalid Mach-O signature command".into()); }
            signature = Some((number(&bytes,cursor+8)?,number(&bytes,cursor+12)?));
            bytes[cursor+8..cursor+16].fill(0);
        }
        if command == 0x19 && size >= 72 && bytes[cursor+8..cursor+24] == *b"__LINKEDIT\0\0\0\0\0\0" {
            bytes[cursor+32..cursor+40].fill(0); bytes[cursor+48..cursor+56].fill(0);
        }
        cursor = next;
    }
    let (offset,size) = signature.ok_or("unsigned Mach-O executable")?;
    if offset < end || size == 0 || offset.checked_add(size) != Some(bytes.len()) { return Err("invalid Mach-O signature range".into()); }
    Ok(hex(&Sha256::digest(&bytes[..offset])))
}
pub fn check_file(path: &Path, artifact: &Artifact) -> Result<(), String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() != artifact.size || !file_hash(path)?.eq_ignore_ascii_case(&artifact.sha256) { return Err("update size/checksum mismatch".into()); }
    Ok(())
}
fn curl(url: &str) -> Command {
    let mut cmd = Command::new("curl");
    cmd.args(["-fsSL", "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "15", "--max-time", "600", "--retry", "2", "-A", "Light-Updater", url]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); }
    cmd
}
pub fn fetch_metadata(url: &str) -> Result<Vec<u8>, String> {
    if url != IDE_FEED && url != MODULE_FEED { return Err("untrusted update feed".into()); }
    let output = curl(url).args(["--max-filesize", "1048576"]).stdout(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 { return Err(format!("update metadata download failed: {}", String::from_utf8_lossy(&output.stderr))); }
    Ok(output.stdout)
}
pub fn download(artifact: &Artifact, cache: &Path) -> Result<PathBuf, String> {
    if !plain(&artifact.name) || !artifact.url.starts_with("https://github.com/sherhn/") || artifact.size > MAX_ARCHIVE { return Err("unsafe update artifact".into()); }
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let path = cache.join(&artifact.name);
    if check_file(&path, artifact).is_ok() { return Ok(path) }
    let temporary = cache.join(format!(".{}.download", artifact.name));
    let result = (|| {
        let output = curl(&artifact.url).args(["--max-filesize", &artifact.size.to_string(), "-o"]).arg(&temporary).output().map_err(|e| e.to_string())?;
        if !output.status.success() { return Err(format!("update download failed: {}", String::from_utf8_lossy(&output.stderr))) }
        check_file(&temporary, artifact)?;
        fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
        Ok(path)
    })();
    if result.is_err() { let _ = fs::remove_file(temporary); }
    result
}
pub fn safe_relative(text: &str) -> bool {
    !text.is_empty() && !text.contains(['\\', ':', '\0']) && text.split('/').all(|part| !part.is_empty() && part != "." && part != "..") && Path::new(text).components().all(|c| matches!(c, Component::Normal(_)))
}
pub fn inventory_path(root: &Path) -> PathBuf {
    let direct = root.join("update-manifest.json");
    if direct.is_file() { direct } else { root.join("Contents/Resources/light-update-manifest.json") }
}
pub fn read_inventory(root: &Path) -> Result<Inventory, String> {
    let data = fs::read(inventory_path(root)).map_err(|e| e.to_string())?;
    if data.len() > 8 * 1024 * 1024 { return Err("update inventory is too large".into()); }
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}
fn auxiliary_owned(name: &str, inventory: &Inventory) -> bool {
    name == "update-manifest.json" || (inventory.entrypoint.starts_with("Contents/MacOS/") && matches!(name, "Contents/Resources/light-update-manifest.json" | "Contents/_CodeSignature/CodeResources"))
}
pub fn validate_tree(root: &Path, expected_version: &str) -> Result<Inventory, String> {
    let inventory = read_inventory(root)?;
    version(&inventory.version)?;
    if inventory.schema != 1 || inventory.version != expected_version || !safe_relative(&inventory.entrypoint) || !inventory.files.contains_key(&inventory.entrypoint) { return Err("update inventory does not match this version".into()); }
    for (name, expected) in &inventory.files {
        if !safe_relative(name) || fs::symlink_metadata(root.join(name)).map_err(|e| e.to_string())?.file_type().is_symlink() { return Err(format!("unsafe update inventory path: {name}")); }
        let actual = if expected.starts_with("macho-sha256:") && name == &inventory.entrypoint && name.starts_with("Contents/MacOS/") { format!("macho-sha256:{}", macho_hash(&root.join(name))?) } else { file_hash(&root.join(name))? };
        if !actual.eq_ignore_ascii_case(expected) { return Err(format!("update inventory mismatch: {name}")); }
    }
    Ok(inventory)
}
pub fn validate_prepared(root: &Path, expected_version: &str) -> Result<Inventory, String> {
    let inventory = validate_tree(root, expected_version)?;
    if tree_files(root)?.keys().any(|name| !inventory.files.contains_key(name) && !auxiliary_owned(name, &inventory)) {
        return Err("unexpected file in prepared update".into());
    }
    Ok(inventory)
}
pub fn unpack(archive: &Path, destination: &Path, expected_version: &str) -> Result<Inventory, String> {
    if destination.exists() {
        let inventory = validate_prepared(destination, expected_version)?;
        // A cache is mutable. Its inventory alone cannot authenticate previously extracted bytes.
        let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let mut names = std::collections::HashSet::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
            let name = entry.name().trim_end_matches('/').to_string();
            if !safe_relative(&name) || !names.insert(name.to_lowercase()) || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) { return Err("unsafe cached update path".into()); }
            if entry.is_dir() { continue }
            let path = destination.join(&name);
            if fs::metadata(&path).map_err(|e| e.to_string())?.len() != entry.size() { return Err("cached update differs from signed archive".into()); }
            let mut hash = Sha256::new(); let mut buffer = [0; 65536];
            loop { let n = entry.read(&mut buffer).map_err(|e| e.to_string())?; if n == 0 { break } hash.update(&buffer[..n]); }
            if file_hash(&path)? != hex(&hash.finalize()) { return Err("cached update differs from signed archive".into()); }
        }
        if tree_files(destination)?.keys().any(|name| !names.contains(&name.to_lowercase())) { return Err("extra cached update file".into()); }
        return Ok(inventory)
    }
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if zip.len() > 50000 { return Err("too many update files".into()); }
        let mut seen = std::collections::HashSet::new(); let mut total = 0_u64;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
            let name = entry.name().trim_end_matches('/');
            if !safe_relative(name) || !seen.insert(name.to_lowercase()) || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) { return Err("unsafe update archive path/link/duplicate".into()); }
            total = total.checked_add(entry.size()).ok_or("update archive too large")?;
            if total > MAX_EXPANDED || entry.size() > MAX_ARCHIVE { return Err("update archive too large".into()); }
            let path = destination.join(name);
            if entry.is_dir() { fs::create_dir_all(&path).map_err(|e| e.to_string())?; continue }
            fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut output = File::create(&path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut output).map_err(|e| e.to_string())?;
            output.sync_all().map_err(|e| e.to_string())?;
            #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&path, fs::Permissions::from_mode(entry.unix_mode().unwrap_or(0o644) & 0o777)).map_err(|e| e.to_string())?; }
        }
        validate_prepared(destination, expected_version)
    })();
    if result.is_err() { let _ = fs::remove_dir_all(destination); }
    result
}
pub fn tree_files(root: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, PathBuf>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?; let path = entry.path();
            if entry.file_type().map_err(|e| e.to_string())?.is_symlink() { return Err("automatic updates do not follow installation symlinks".into()); }
            if path.is_dir() { walk(root, &path, out)?; } else { out.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), path); }
        }
        Ok(())
    }
    let mut result = BTreeMap::new(); walk(root, root, &mut result)?; Ok(result)
}

/// Locks live outside installation directories so their handles cannot prevent renaming them.
pub fn lock(path: &Path, exclusive: bool) -> Result<File, String> {
    fs::create_dir_all(path.parent().ok_or("lock has no parent")?).map_err(|e| e.to_string())?;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(|e| e.to_string())?;
    if exclusive { fs2::FileExt::try_lock_exclusive(&file) } else { fs2::FileExt::try_lock_shared(&file) }.map_err(|_| "another Light instance is still using these files")?;
    Ok(file)
}

fn copy_files(source: &Path, target: &Path) -> Result<(), String> {
    for (relative, path) in tree_files(source)? {
        let destination = target.join(relative); fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(path, destination).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Never mutate the active directory until the new tree and preserved user files are complete.
/// A failed swap or health check restores the complete previous tree.
pub fn replace(installation: &Path, prepared: &Path, expected_version: &str, mut health_check: impl FnMut(&Path) -> Result<(), String>) -> Result<PathBuf, String> {
    let root = installation.canonicalize().map_err(|e| e.to_string())?;
    let parent = root.parent().ok_or("refusing to replace a filesystem root")?;
    let current = read_inventory(&root)?;
    validate_tree(&root, &current.version).map_err(|e| format!("installation was modified; automatic replacement refused: {e}"))?;
    let next = validate_prepared(prepared, expected_version)?;
    if next.entrypoint != current.entrypoint { return Err("update uses a different installation layout; use its native installer".into()); }
    if version(expected_version)? <= version(&current.version)? { return Err("refusing a stale update/downgrade".into()); }
    let stem = root.file_name().unwrap().to_string_lossy();
    let fresh = parent.join(format!(".{stem}.updating-{}", std::process::id()));
    let backup = parent.join(format!(".{stem}.previous-{}", current.version));
    if fresh.exists() || backup.exists() { return Err("an update/backup already exists; leaving the installation untouched".into()); }
    fs::create_dir(&fresh).map_err(|e| e.to_string())?;
    let prepare = (|| {
        copy_files(prepared, &fresh)?;
        for (relative, path) in tree_files(&root)? {
            if auxiliary_owned(&relative, &current) || current.files.contains_key(&relative) { continue }
            if next.files.contains_key(&relative) || auxiliary_owned(&relative, &next) { return Err(format!("a user file conflicts with the update: {relative}")); }
            let target = fresh.join(relative); fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?; fs::copy(path, target).map_err(|e| e.to_string())?;
        }
        validate_tree(&fresh, expected_version)?; Ok(())
    })();
    if let Err(e) = prepare { let _ = fs::remove_dir_all(&fresh); return Err(e) }
    if let Err(error) = health_check(&fresh.join(&next.entrypoint)) {
        let _ = fs::remove_dir_all(&fresh);
        return Err(format!("update preflight failed; original installation unchanged: {error}"));
    }
    if let Err(e) = fs::rename(&root, &backup) { let _ = fs::remove_dir_all(&fresh); return Err(format!("installation is busy; update deferred: {e}")) }
    if let Err(e) = fs::rename(&fresh, &root) { let _ = fs::rename(&backup, &root); return Err(e.to_string()) }
    if let Err(error) = health_check(&root.join(&next.entrypoint)) {
        let failed = parent.join(format!(".{stem}.failed-{}", std::process::id()));
        fs::rename(&root, &failed).map_err(|e| format!("health check failed: {error}; cannot move failed tree: {e}"))?;
        fs::rename(&backup, &root).map_err(|e| format!("health check failed: {error}; restore failed: {e}"))?;
        let _ = fs::remove_dir_all(failed); return Err(format!("update rolled back: {error}"));
    }
    Ok(backup)
}

/// Explicit user-requested rollback. The backup must be our sibling slot and pass inventory validation.
pub fn restore(installation: &Path, previous: &Path, health_check: impl FnOnce(&Path) -> Result<(), String>) -> Result<PathBuf, String> {
    let root = installation.canonicalize().map_err(|e| e.to_string())?;
    let previous = previous.canonicalize().map_err(|e| e.to_string())?;
    let parent = root.parent().ok_or("cannot roll back a filesystem root")?;
    let current = read_inventory(&root)?; version(&current.version)?;
    let older = read_inventory(&previous)?; validate_tree(&previous, &older.version)?;
    let stem = root.file_name().ok_or("installation has no name")?.to_string_lossy();
    if previous.parent() != Some(parent) || previous.file_name().unwrap() != format!(".{stem}.previous-{}", older.version).as_str() { return Err("rollback source is not this installation's previous slot".into()); }
    let saved = parent.join(format!(".{stem}.previous-{}", current.version));
    if saved.exists() { return Err("rollback destination already exists".into()); }
    for (relative, path) in tree_files(&root)? {
        if auxiliary_owned(&relative, &current) || current.files.contains_key(&relative) { continue }
        if older.files.contains_key(&relative) { return Err("a user file conflicts with a previous-version program file".into()); }
        let target = previous.join(relative); fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?; fs::copy(path, target).map_err(|e| e.to_string())?;
    }
    fs::rename(&root, &saved).map_err(|e| e.to_string())?;
    if let Err(e) = fs::rename(&previous, &root) { let _ = fs::rename(&saved, &root); return Err(e.to_string()); }
    if let Err(error) = health_check(&root.join(&older.entrypoint)) {
        fs::rename(&root, &previous).map_err(|e| e.to_string())?;
        fs::rename(&saved, &root).map_err(|e| e.to_string())?;
        return Err(format!("rollback startup check failed; current installation restored: {error}"));
    }
    Ok(saved)
}

pub fn atomic_json(path: &Path, data: &impl Serialize) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("state has no parent")?).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let mut file = File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?; drop(file);
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use crate::testutil::TempDir;

    fn manifest() -> Manifest {
        Manifest { schema: 1, kind: "ide".into(), version: "1.3.0".into(), issued: 1000, expires: 2000,
            targets: BTreeMap::from([("windows-x64".into(), Artifact { name: "Light-1.3.0-windows-x64-update.zip".into(), url: "https://github.com/sherhn/Light/releases/download/v1.3.0/Light-1.3.0-windows-x64-update.zip".into(), size: 10, sha256: "ab".repeat(32) })]) }
    }
    fn signed(value: &Manifest) -> (Vec<u8>, String) {
        // Public, deterministic test fixture; never used for production packages.
        let key = SigningKey::from_bytes(&[42; 32]);
        let text = serde_json::to_string(value).unwrap();
        let envelope = Envelope { signature: hex(&key.sign(text.as_bytes()).to_bytes()), signed: text };
        (serde_json::to_vec(&envelope).unwrap(), hex(key.verifying_key().as_bytes()))
    }
    #[test]
    fn authenticity_expiry_target_origin_and_version_are_required() {
        let value = manifest(); let (bytes, key) = signed(&value);
        assert_eq!(verify(&bytes, &key, "ide", 1100).unwrap().version, "1.3.0");
        assert!(verify(&bytes, &key, "ide", 2000).is_err());
        assert!(verify(&bytes, &key, "ide", 1).is_err());
        assert!(verify(&bytes, &key, "modules", 1100).is_err());
        assert!(verify(&bytes, &"00".repeat(32), "ide", 1100).is_err());
        let mut envelope: Envelope = serde_json::from_slice(&bytes).unwrap();
        envelope.signed = envelope.signed.replace("1.3.0", "9.9.9");
        assert!(verify(&serde_json::to_vec(&envelope).unwrap(), &key, "ide", 1100).is_err());
        for bad in ["https://evil.test/update.zip", "http://github.com/sherhn/Light/update.zip", "https://github.com/sherhn/Light/releases/download/v1.3.0/../evil.zip"] {
            let mut value = value.clone(); value.targets.get_mut("windows-x64").unwrap().url = bad.into();
            let (bytes, key) = signed(&value); assert!(verify(&bytes, &key, "ide", 1100).is_err());
        }
        assert!(version("1.3.0-preview").is_err()); assert!(version("1.3").is_err());
        assert!(version("1.10.0").unwrap() > version("1.9.9").unwrap());
    }
    #[test]
    fn python_release_signatures_are_verified_by_the_native_client() {
        let key = SigningKey::from_bytes(&[42; 32]);
        let fixture = include_bytes!("../tests/fixtures/signed-update.json");
        assert_eq!(verify(fixture, &hex(key.verifying_key().as_bytes()), "ide", 1100).unwrap().version, "1.3.0");
    }
    #[test]
    fn python_macho_hashes_survive_resigning_and_reject_code_edits_and_bad_ranges() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("../tests/fixtures/macho-hash.json")).unwrap();
        let d = TempDir::new("macho-hash"); let path = d.path().join("light");
        let decode = |text: &str| text.as_bytes().as_chunks::<2>().0.iter().map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(),16).unwrap()).collect::<Vec<_>>();
        for key in ["bytes", "resigned"] {
            let bytes = decode(fixture[key].as_str().unwrap()); fs::write(&path,&bytes).unwrap();
            assert_eq!(macho_hash(&path).unwrap(), fixture["sha256"].as_str().unwrap());
            let mut changed=bytes.clone(); changed[200]^=1; fs::write(&path,changed).unwrap();
            assert_ne!(macho_hash(&path).unwrap(), fixture["sha256"].as_str().unwrap());
            fs::write(&path,&bytes[..bytes.len()-1]).unwrap(); assert!(macho_hash(&path).is_err());
        }
    }
    fn tree(root: &Path, version: &str, content: &str) {
        fs::create_dir_all(root).unwrap(); fs::write(root.join("light.exe"), content).unwrap();
        atomic_json(&root.join("update-manifest.json"), &Inventory { schema: 1, version: version.into(), entrypoint: "light.exe".into(), files: BTreeMap::from([("light.exe".into(), file_hash(&root.join("light.exe")).unwrap())]) }).unwrap();
    }
    #[test]
    fn successful_upgrade_preserves_user_files_and_previous_version() {
        let d = TempDir::new("updates"); let root = d.path().join("Light"); let stage = d.path().join("stage");
        tree(&root, "1.2.0", "old"); tree(&stage, "1.3.0", "new");
        fs::write(root.join("my-notes.md"), "keep me").unwrap();
        let backup = replace(&root, &stage, "1.3.0", |_| Ok(())).unwrap();
        assert_eq!(fs::read_to_string(root.join("light.exe")).unwrap(), "new");
        assert_eq!(fs::read_to_string(root.join("my-notes.md")).unwrap(), "keep me");
        assert_eq!(fs::read_to_string(backup.join("light.exe")).unwrap(), "old");
        assert!(replace(&root, &stage, "1.3.0", |_| Ok(())).is_err());
    }
    #[test]
    fn failed_health_check_restores_every_original_file() {
        let d = TempDir::new("update-rollback"); let root = d.path().join("Light"); let stage = d.path().join("stage");
        tree(&root, "1.2.0", "old"); tree(&stage, "1.3.0", "new"); fs::write(root.join("private.txt"), "retained").unwrap();
        let mut calls = 0;
        assert!(replace(&root, &stage, "1.3.0", |_| { calls += 1; if calls == 1 { Ok(()) } else { Err("simulated startup crash".into()) } }).unwrap_err().contains("rolled back"));
        assert_eq!(fs::read_to_string(root.join("light.exe")).unwrap(), "old");
        assert_eq!(fs::read_to_string(root.join("private.txt")).unwrap(), "retained");
        validate_tree(&root, "1.2.0").unwrap();
    }
    #[test]
    fn customized_installations_and_corrupt_staging_are_never_replaced() {
        let d = TempDir::new("update-local-edits"); let root = d.path().join("Light"); let stage = d.path().join("stage");
        tree(&root, "1.2.0", "old"); tree(&stage, "1.3.0", "new");
        fs::write(root.join("light.exe"), "my custom build").unwrap();
        assert!(replace(&root, &stage, "1.3.0", |_| Ok(())).is_err());
        assert_eq!(fs::read_to_string(root.join("light.exe")).unwrap(), "my custom build");
        tree(&root, "1.2.0", "old"); fs::write(stage.join("light.exe"), "corrupt").unwrap();
        assert!(replace(&root, &stage, "1.3.0", |_| Ok(())).is_err());
        assert_eq!(fs::read_to_string(root.join("light.exe")).unwrap(), "old");
    }
    #[test]
    fn prepared_cache_is_rechecked_against_archive_even_if_inventory_was_rewritten() {
        use zip::write::SimpleFileOptions;
        let d = TempDir::new("update-cache"); let source = d.path().join("source");
        tree(&source, "1.3.0", "authentic");
        let archive = d.path().join("update.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        for (name, path) in tree_files(&source).unwrap() {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(&fs::read(path).unwrap()).unwrap();
        }
        zip.finish().unwrap();
        let cache = d.path().join("cached");
        unpack(&archive, &cache, "1.3.0").unwrap();
        unpack(&archive, &cache, "1.3.0").unwrap();
        fs::write(cache.join("unexpected.dll"), "injected").unwrap();
        assert!(unpack(&archive, &cache, "1.3.0").is_err());
        fs::remove_file(cache.join("unexpected.dll")).unwrap();
        tree(&cache, "1.3.0", "tampered!");
        assert!(validate_prepared(&cache, "1.3.0").is_ok());
        assert!(unpack(&archive, &cache, "1.3.0").unwrap_err().contains("signed archive"));
    }
    #[test]
    fn preflight_failure_and_user_file_collisions_leave_the_installation_untouched() {
        let d = TempDir::new("update-preflight"); let root = d.path().join("Light"); let stage = d.path().join("stage");
        tree(&root, "1.2.0", "old"); tree(&stage, "1.3.0", "new");
        assert!(replace(&root, &stage, "1.3.0", |_| Err("missing runtime".into())).unwrap_err().contains("preflight"));
        validate_tree(&root, "1.2.0").unwrap();
        assert!(!d.path().join(".Light.previous-1.2.0").exists());
        fs::write(root.join("plugin.dll"), "user plugin").unwrap();
        fs::write(stage.join("plugin.dll"), "new program component").unwrap();
        let mut inventory = read_inventory(&stage).unwrap(); inventory.files.insert("plugin.dll".into(), file_hash(&stage.join("plugin.dll")).unwrap());
        atomic_json(&inventory_path(&stage), &inventory).unwrap();
        assert!(replace(&root, &stage, "1.3.0", |_| Ok(())).unwrap_err().contains("conflicts"));
        validate_tree(&root, "1.2.0").unwrap();
        assert_eq!(fs::read_to_string(root.join("plugin.dll")).unwrap(), "user plugin");
    }
    #[test]
    fn paths_and_cross_process_locks_protect_active_installations() {
        for bad in ["../x", "/x", "C:/x", "a\\b", "a//b", "a/./b", "a/../b", "x\0y"] { assert!(!safe_relative(bad), "{bad:?}"); }
        assert!(safe_relative("Contents/MacOS/light"));
        let d = TempDir::new("update-lock"); let path = d.path().join("locks/app.lock");
        let first = lock(&path, false).unwrap(); let second = lock(&path, false).unwrap();
        assert!(lock(&path, true).is_err()); drop(first); drop(second);
        assert!(lock(&path, true).is_ok());
    }
    #[test]
    fn explicit_rollback_keeps_notes_created_after_the_update() {
        let d = TempDir::new("update-user-rollback"); let root = d.path().join("Light"); let stage = d.path().join("stage");
        tree(&root, "1.2.0", "old"); tree(&stage, "1.3.0", "new");
        let previous = replace(&root, &stage, "1.3.0", |_| Ok(())).unwrap();
        fs::write(root.join("latest-notes.md"), "written after updating").unwrap();
        let next = restore(&root, &previous, |_| Ok(())).unwrap();
        assert_eq!(fs::read_to_string(root.join("light.exe")).unwrap(), "old");
        assert_eq!(fs::read_to_string(root.join("latest-notes.md")).unwrap(), "written after updating");
        assert_eq!(fs::read_to_string(next.join("light.exe")).unwrap(), "new");
        let alien = d.path().join("unrelated"); tree(&alien, "1.1.0", "alien");
        assert!(restore(&root, &alien, |_| Ok(())).is_err());
    }
    #[test]
    fn unsafe_duplicate_and_unexpected_zip_entries_are_rejected_before_activation() {
        use zip::write::SimpleFileOptions;
        let d = TempDir::new("update-zip");
        for names in [vec!["../outside"], vec!["a.txt", "A.txt"], vec!["C:/outside"], vec!["a/./outside"]] {
            let path = d.path().join("bad.zip");
            let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
            for name in names { zip.start_file(name, SimpleFileOptions::default()).unwrap(); zip.write_all(b"bad").unwrap(); }
            zip.finish().unwrap();
            assert!(unpack(&path, &d.path().join("unpacked"), "1.3.0").is_err());
            assert!(!d.path().join("outside").exists()); assert!(!d.path().join("unpacked").exists());
        }
    }
}
