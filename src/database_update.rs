use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, bail};

const INSTALLED_DIR: &str = "installed";

#[derive(Debug, Clone)]
pub struct DatabaseInboxState {
    pub root: PathBuf,
    pub installed: Option<PathBuf>,
    pub candidate: Option<PathBuf>,
}

impl DatabaseInboxState {
    pub fn action_label(&self) -> &'static str {
        if self.installed.is_some() {
            "UPDATE"
        } else {
            "INSTALL"
        }
    }
}

pub fn database_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("StarSync MinerScan").join("database")
}

pub fn installed_database_dir() -> PathBuf {
    database_root().join(INSTALLED_DIR)
}

pub fn ensure_database_root() -> Result<PathBuf> {
    let root = database_root();
    fs::create_dir_all(&root)
        .with_context(|| format!("Could not create database directory {}", root.display()))?;
    Ok(root)
}

pub fn installed_dataset() -> Option<PathBuf> {
    find_dataset_root(&installed_database_dir(), 4)
}

pub fn inbox_state() -> DatabaseInboxState {
    let root = database_root();
    let installed = installed_dataset();
    let candidate = discover_candidate(&root);
    DatabaseInboxState {
        root,
        installed,
        candidate,
    }
}

pub fn install_or_update_candidate() -> Result<PathBuf> {
    let root = ensure_database_root()?;
    let candidate = discover_candidate(&root)
        .context("No SCUnpacked ZIP or extracted dataset found in the database directory")?;
    let installed = installed_database_dir();
    let staging = root.join(".install-staging");
    if staging.exists() {
        fs::remove_dir_all(&staging).context("Could not clear database staging directory")?;
    }
    fs::create_dir_all(&staging)?;

    let source_root = if candidate.is_file() {
        extract_zip(&candidate, &staging)?;
        find_dataset_root(&staging, 5).context(
            "The ZIP does not contain a valid SCUnpacked resources/commodities.json dataset",
        )?
    } else {
        find_dataset_root(&candidate, 5)
            .context("The selected database folder does not contain resources/commodities.json")?
    };

    let replacement = root.join(".installed-new");
    if replacement.exists() {
        fs::remove_dir_all(&replacement)?;
    }
    copy_dir_recursive(&source_root, &replacement)?;
    validate_dataset(&replacement)?;

    if installed.exists() {
        fs::remove_dir_all(&installed).context("Could not replace installed SCUnpacked dataset")?;
    }
    fs::rename(&replacement, &installed)
        .context("Could not activate installed SCUnpacked dataset")?;
    let _ = fs::remove_dir_all(&staging);

    find_dataset_root(&installed, 2).context("Installed SCUnpacked dataset could not be resolved")
}

pub fn validate_dataset(root: &Path) -> Result<()> {
    let dataset = find_dataset_root(root, 5)
        .with_context(|| format!("No SCUnpacked dataset found below {}", root.display()))?;
    let resources = dataset.join("resources");
    for required in ["commodities.json", "commodity_trade_locations.json"] {
        let path = resources.join(required);
        if !path.is_file() {
            bail!("Missing required SCUnpacked file: {}", path.display());
        }
    }
    Ok(())
}

fn discover_candidate(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case(INSTALLED_DIR)
                    || name.starts_with(".install-")
                    || name.eq_ignore_ascii_case(".installed-new")
            })
        {
            continue;
        }
        let valid = if path.is_file() {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        } else if path.is_dir() {
            find_dataset_root(&path, 4).is_some()
        } else {
            false
        };
        if valid {
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            candidates.push((modified, path));
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.into_iter().next().map(|(_, path)| path)
}

fn find_dataset_root(root: &Path, depth: usize) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    if root.join("resources").join("commodities.json").is_file() {
        return Some(root.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_dataset_root(&path, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

fn extract_zip(zip_path: &Path, destination: &Path) -> Result<()> {
    let file = fs::File::open(zip_path)
        .with_context(|| format!("Could not open {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("Invalid SCUnpacked ZIP archive")?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let output = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&output)?;
        io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let src = entry.path();
        let dst = destination.join(entry.file_name());
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else if src.is_file() {
            fs::copy(&src, &dst).with_context(|| {
                format!("Could not copy {} to {}", src.display(), dst.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_source_shape_is_valid() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/scunpacked-4.10");
        validate_dataset(&root).unwrap();
    }

    #[test]
    fn zip_extraction_accepts_scunpacked_shape() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let unique = format!(
            "minerscan-db-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let temp = std::env::temp_dir().join(unique);
        fs::create_dir_all(&temp).unwrap();
        let zip_path = temp.join("scunpacked.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        writer
            .start_file("scunpacked-data/resources/commodities.json", options)
            .unwrap();
        writer.write_all(b"[]").unwrap();
        writer
            .start_file(
                "scunpacked-data/resources/commodity_trade_locations.json",
                options,
            )
            .unwrap();
        writer.write_all(b"[]").unwrap();
        writer.finish().unwrap();

        let extracted = temp.join("extracted");
        fs::create_dir_all(&extracted).unwrap();
        extract_zip(&zip_path, &extracted).unwrap();
        let dataset = find_dataset_root(&extracted, 4).expect("extracted dataset root");
        validate_dataset(&dataset).unwrap();
        let _ = fs::remove_dir_all(temp);
    }
}
