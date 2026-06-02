//! `/api/fs/list`: read-only directory listing that powers the web portal's
//! folder picker for adding a repo. The browser cannot read absolute paths, so
//! the server enumerates subdirectories and returns absolute paths the client
//! can navigate and then submit to `POST /api/repos`.

use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::path::{Utf8Path, Utf8PathBuf};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct FsEntry {
    pub name: String,
    pub path: Utf8PathBuf,
    pub has_git: bool,
    pub hidden: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct FsListing {
    pub path: Utf8PathBuf,
    pub parent: Option<Utf8PathBuf>,
    pub entries: Vec<FsEntry>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FsListError {
    NotFound,
    NotADirectory,
    PermissionDenied,
    NonUtf8,
}

/// List the immediate subdirectories of `path` (files excluded), sorted
/// case-insensitively by name. Hidden (dot-prefixed) directories are included
/// only when `show_hidden` is true.
pub(crate) fn list_directory(path: &Utf8Path, show_hidden: bool) -> Result<FsListing, FsListError> {
    use std::io::ErrorKind;

    let canonical = dunce::canonicalize(path.as_std_path()).map_err(|e| match e.kind() {
        ErrorKind::NotFound => FsListError::NotFound,
        ErrorKind::PermissionDenied => FsListError::PermissionDenied,
        _ => FsListError::NotFound,
    })?;
    let canonical = Utf8PathBuf::from_path_buf(canonical).map_err(|_| FsListError::NonUtf8)?;

    if !canonical.is_dir() {
        return Err(FsListError::NotADirectory);
    }

    let read = std::fs::read_dir(canonical.as_std_path()).map_err(|e| match e.kind() {
        ErrorKind::PermissionDenied => FsListError::PermissionDenied,
        ErrorKind::NotFound => FsListError::NotFound,
        _ => FsListError::PermissionDenied,
    })?;

    let mut entries: Vec<FsEntry> = Vec::new();
    for dirent in read.flatten() {
        let Ok(name) = dirent.file_name().into_string() else {
            continue;
        };
        let entry_path = canonical.join(&name);
        if !entry_path.as_std_path().is_dir() {
            continue;
        }
        let hidden = name.starts_with('.');
        if hidden && !show_hidden {
            continue;
        }
        let has_git = entry_path.join(".git").as_std_path().exists();
        entries.push(FsEntry {
            name,
            path: entry_path,
            has_git,
            hidden,
        });
    }
    entries.sort_by_key(|a| a.name.to_lowercase());

    let parent = canonical.parent().map(|p| p.to_path_buf());

    Ok(FsListing {
        path: canonical,
        parent,
        entries,
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct FsListParams {
    path: Option<String>,
    show_hidden: Option<bool>,
}

fn default_home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

/// `GET /api/fs/list?path=&show_hidden=` -> directory listing JSON.
pub(crate) async fn handle_fs_list(
    Query(params): Query<FsListParams>,
) -> Result<Response, super::ApiError> {
    let show_hidden = params.show_hidden.unwrap_or(false);
    let raw = params
        .path
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(default_home_dir);

    let path = Utf8PathBuf::from(raw);
    let result = tokio::task::spawn_blocking(move || list_directory(&path, show_hidden))
        .await
        .map_err(|e| super::ApiError(format!("fs list task join error: {e}")))?;

    match result {
        Ok(listing) => Ok(Json(listing).into_response()),
        Err(err) => {
            let (status, msg) = match err {
                FsListError::NotFound => (StatusCode::BAD_REQUEST, "path not found"),
                FsListError::NotADirectory => (StatusCode::BAD_REQUEST, "path is not a directory"),
                FsListError::NonUtf8 => (StatusCode::BAD_REQUEST, "path is not valid UTF-8"),
                FsListError::PermissionDenied => (StatusCode::FORBIDDEN, "permission denied"),
            };
            Ok((status, Json(json!({ "error": msg }))).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn utf8(p: std::path::PathBuf) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(p).expect("utf8 temp path")
    }

    #[test]
    fn lists_subdirs_sorted_excludes_files() {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("beta")).unwrap();
        fs::create_dir(tmp.path().join("Alpha")).unwrap();
        fs::write(tmp.path().join("a_file.txt"), b"x").unwrap();

        let listing = list_directory(&utf8(tmp.path().to_path_buf()), false).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "beta"]);
    }

    #[test]
    fn hidden_dirs_excluded_by_default_included_with_flag() {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join(".hidden")).unwrap();
        fs::create_dir(tmp.path().join("visible")).unwrap();

        let base = utf8(tmp.path().to_path_buf());
        let default = list_directory(&base, false).unwrap();
        assert_eq!(
            default
                .entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["visible"]
        );

        let with_hidden = list_directory(&base, true).unwrap();
        let names: Vec<&str> = with_hidden
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec![".hidden", "visible"]);
        assert!(
            with_hidden
                .entries
                .iter()
                .find(|e| e.name == ".hidden")
                .unwrap()
                .hidden
        );
    }

    #[test]
    fn detects_has_git() {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("repo")).unwrap();
        fs::create_dir(tmp.path().join("repo").join(".git")).unwrap();
        fs::create_dir(tmp.path().join("plain")).unwrap();

        let listing = list_directory(&utf8(tmp.path().to_path_buf()), false).unwrap();
        let repo = listing.entries.iter().find(|e| e.name == "repo").unwrap();
        let plain = listing.entries.iter().find(|e| e.name == "plain").unwrap();
        assert!(repo.has_git);
        assert!(!plain.has_git);
    }

    #[test]
    fn parent_is_some_for_nested_dir() {
        let tmp = tempdir().unwrap();
        let child = tmp.path().join("child");
        fs::create_dir(&child).unwrap();
        let listing = list_directory(&utf8(child), false).unwrap();
        assert!(listing.parent.is_some());
    }

    #[test]
    fn root_has_no_parent() {
        let listing = list_directory(Utf8Path::new("/"), false).unwrap();
        assert_eq!(listing.parent, None);
    }

    #[test]
    fn nonexistent_path_is_not_found() {
        let err = list_directory(Utf8Path::new("/no/such/path/xyzzy-9q"), false).unwrap_err();
        assert_eq!(err, FsListError::NotFound);
    }

    #[test]
    fn file_path_is_not_a_directory() {
        let tmp = tempdir().unwrap();
        let file = tmp.path().join("f.txt");
        fs::write(&file, b"x").unwrap();
        let err = list_directory(&utf8(file), false).unwrap_err();
        assert_eq!(err, FsListError::NotADirectory);
    }

    #[tokio::test]
    async fn handler_lists_explicit_path() {
        use axum::extract::Query;
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let params = FsListParams {
            path: Some(tmp.path().to_str().unwrap().to_string()),
            show_hidden: Some(false),
        };
        let resp = handle_fs_list(Query(params)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn handler_maps_missing_path_to_400() {
        use axum::extract::Query;
        let params = FsListParams {
            path: Some("/no/such/path/xyzzy-9q".to_string()),
            show_hidden: None,
        };
        let resp = handle_fs_list(Query(params)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
