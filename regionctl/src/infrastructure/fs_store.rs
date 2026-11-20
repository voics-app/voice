//! The gitops checkout on disk.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::application::error::AppError;
use crate::application::ports::RegionStore;

// These directories compose shared or environment-level resources, not node overlays.
// `dev` is the GitOps aggregate referencing common and nl-dev; do not allocate it a worker.
const NOT_REGIONS: [&str; 3] = ["_template", "common", "dev"];
const OVERLAY_FILE: &str = "kustomization.yaml";

/// `RegionStore` over `<gitops-dir>/regions`.
#[derive(Debug)]
pub struct FsRegionStore {
    regions_dir: PathBuf,
}

fn store_error(path: &Path, err: &std::io::Error) -> AppError {
    AppError::Store(format!("{}: {err}", path.display()))
}

impl FsRegionStore {
    /// Opens the store; `gitops_dir` must contain a `regions` directory.
    pub fn open(gitops_dir: &Path) -> Result<Self, AppError> {
        let regions_dir = gitops_dir.join("regions");
        if !regions_dir.is_dir() {
            return Err(AppError::Store(format!(
                "{} has no regions/ directory; point --gitops-dir at a checkout of discordy-voics/gitops",
                gitops_dir.display()
            )));
        }
        Ok(Self { regions_dir })
    }

    fn overlay_path(&self, name: &str) -> PathBuf {
        self.regions_dir.join(name).join(OVERLAY_FILE)
    }

    fn root_path(&self) -> PathBuf {
        self.regions_dir.join(OVERLAY_FILE)
    }
}

impl RegionStore for FsRegionStore {
    fn names(&self) -> Result<Vec<String>, AppError> {
        let entries =
            fs::read_dir(&self.regions_dir).map_err(|e| store_error(&self.regions_dir, &e))?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| store_error(&self.regions_dir, &e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && !NOT_REGIONS.contains(&name.as_str()) {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn entry_exists(&self, name: &str) -> Result<bool, AppError> {
        let path = self.regions_dir.join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
            Err(err) => Err(store_error(&path, &err)),
        }
    }

    fn read_overlay(&self, name: &str) -> Result<Option<String>, AppError> {
        let path = self.overlay_path(name);
        match fs::read_to_string(&path) {
            Ok(content) => Ok(Some(content)),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(store_error(&path, &err)),
        }
    }

    fn write_overlay(&self, name: &str, content: &str) -> Result<(), AppError> {
        let path = self.overlay_path(name);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| store_error(dir, &e))?;
        }
        fs::write(&path, content).map_err(|e| store_error(&path, &e))
    }

    fn read_root(&self) -> Result<String, AppError> {
        let path = self.root_path();
        fs::read_to_string(&path).map_err(|e| store_error(&path, &e))
    }

    fn write_root(&self, content: &str) -> Result<(), AppError> {
        let path = self.root_path();
        fs::write(&path, content).map_err(|e| store_error(&path, &e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_a_regions_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            FsRegionStore::open(dir.path()),
            Err(AppError::Store(_))
        ));
    }

    #[test]
    fn reads_and_writes_overlays_and_skips_shared_layers() {
        let dir = tempfile::tempdir().unwrap();
        let regions = dir.path().join("regions");
        for sub in ["_template", "common", "dev", "nl-home"] {
            fs::create_dir_all(regions.join(sub)).unwrap();
        }
        fs::write(
            regions.join("kustomization.yaml"),
            "resources:\n  - common\n",
        )
        .unwrap();

        let store = FsRegionStore::open(dir.path()).unwrap();
        assert_eq!(store.names().unwrap(), vec!["nl-home".to_owned()]);
        assert_eq!(store.read_overlay("nl-home").unwrap(), None);

        store.write_overlay("eu-west", "x: 1\n").unwrap();
        assert_eq!(
            store.read_overlay("eu-west").unwrap().as_deref(),
            Some("x: 1\n")
        );
        assert_eq!(
            store.names().unwrap(),
            vec!["eu-west".to_owned(), "nl-home".to_owned()]
        );

        store.write_root("resources: []\n").unwrap();
        assert_eq!(store.read_root().unwrap(), "resources: []\n");
    }
}
