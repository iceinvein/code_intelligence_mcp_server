use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

fn has_required_model_files(model_dir: &Path) -> bool {
    if !model_dir.is_dir() {
        return false;
    }
    let config = model_dir.join("config.json");
    let tokenizer = model_dir.join("tokenizer.json");
    if !(config.is_file() && tokenizer.is_file()) {
        return false;
    }
    let single = model_dir.join("model.safetensors");
    let index = model_dir.join("model.safetensors.index.json");
    if single.is_file() || index.is_file() {
        return true;
    }
    if let Ok(entries) = fs::read_dir(model_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("safetensors") && p.is_file() {
                return true;
            }
        }
    }
    false
}

fn staging_dir(dest_dir: &Path) -> Result<PathBuf> {
    let parent = dest_dir
        .parent()
        .ok_or_else(|| anyhow!("Invalid model dir: {}", dest_dir.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    Ok(parent.join(format!(
        ".embeddings-download-{}-{}",
        std::process::id(),
        nanos
    )))
}

fn safe_join_under(base: &Path, rel: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::from(base);
    for c in rel.components() {
        match c {
            std::path::Component::Normal(p) => out.push(p),
            std::path::Component::CurDir => {}
            _ => return Err(anyhow!("Refusing to unpack path: {}", rel.display())),
        }
    }
    Ok(out)
}

fn maybe_promote_single_subdir(dest_dir: &Path) -> Result<()> {
    if has_required_model_files(dest_dir) {
        return Ok(());
    }

    let mut entries = fs::read_dir(dest_dir)
        .with_context(|| format!("Failed to read {}", dest_dir.display()))?
        .flatten()
        .collect::<Vec<_>>();
    if entries.len() != 1 {
        return Ok(());
    }
    let only = entries.remove(0);
    let path = only.path();
    if !path.is_dir() {
        return Ok(());
    }

    if !has_required_model_files(&path) {
        return Ok(());
    }

    let tmp = staging_dir(dest_dir)?;
    fs::create_dir_all(&tmp).with_context(|| format!("Failed to create {}", tmp.display()))?;

    for e in fs::read_dir(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?
        .flatten()
    {
        let from = e.path();
        let name = from
            .file_name()
            .ok_or_else(|| anyhow!("Invalid file name: {}", from.display()))?
            .to_os_string();
        let to = tmp.join(name);
        fs::rename(&from, &to).or_else(|_| -> Result<()> {
            if from.is_dir() {
                copy_dir_all(&from, &to)?;
                fs::remove_dir_all(&from).ok();
            } else {
                fs::copy(&from, &to)?;
                fs::remove_file(&from).ok();
            }
            Ok(())
        })?;
    }

    fs::remove_dir_all(dest_dir)
        .with_context(|| format!("Failed to remove {}", dest_dir.display()))?;
    fs::rename(&tmp, dest_dir).with_context(|| {
        format!(
            "Failed to move promoted model dir into place: {}",
            dest_dir.display()
        )
    })?;
    Ok(())
}

fn copy_dir_all(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("Failed to create {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("Failed to read {}", from.display()))? {
        let entry = entry.with_context(|| format!("Failed to read entry in {}", from.display()))?;
        let path = entry.path();
        let name = path
            .file_name()
            .ok_or_else(|| anyhow!("Invalid file name: {}", path.display()))?
            .to_os_string();
        let dest = to.join(name);
        if path.is_dir() {
            copy_dir_all(&path, &dest)?;
        } else {
            fs::copy(&path, &dest).with_context(|| {
                format!("Failed to copy {} to {}", path.display(), dest.display())
            })?;
        }
    }
    Ok(())
}

fn unpack_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = fs::File::open(archive_path)
        .with_context(|| format!("Failed to open {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().context("Failed to read tar entries")? {
        let mut entry = entry.context("Failed to read tar entry")?;
        let rel = entry
            .path()
            .context("Failed to read entry path")?
            .to_path_buf();
        let out = safe_join_under(dest_dir, &rel)?;
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        entry
            .unpack(&out)
            .with_context(|| format!("Failed to unpack {}", rel.display()))?;
    }
    Ok(())
}

async fn download_to_file(url: &str, path: &Path) -> Result<String> {
    let client = reqwest::Client::builder()
        .default_headers(default_headers(None)?)
        .build()
        .context("Failed to build HTTP client")?;

    download_to_file_with_client(&client, url, path).await
}

fn default_headers(hf_token: Option<&str>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("code-intelligence-mcp-server/0.1 (+https://github.com)"),
    );
    if let Some(token) = hf_token {
        let value = format!("Bearer {}", token.trim());
        let value = HeaderValue::from_str(&value).context("Invalid EMBEDDINGS_MODEL_HF_TOKEN")?;
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

async fn download_to_file_with_client(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .context("Failed to download model")?;
    if !resp.status().is_success() {
        return Err(anyhow!("Failed to download model: HTTP {}", resp.status()));
    }

    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("Failed to create {}", path.display()))?;
    let mut hasher = Sha256::new();

    let mut stream = resp.bytes_stream();
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Failed to read download stream")?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .context("Failed to write download to disk")?;
    }
    file.flush().await.ok();

    Ok(hex::encode(hasher.finalize()))
}

pub async fn ensure_candle_model(
    model_dir: &Path,
    url: &str,
    sha256_hex: Option<&str>,
) -> Result<bool> {
    if has_required_model_files(model_dir) {
        debug!(model_dir = %model_dir.display(), "Embeddings model already present");
        return Ok(false);
    }

    info!(
        model_dir = %model_dir.display(),
        url = %url,
        "Downloading embeddings model archive"
    );

    if !(url.ends_with(".tar.gz") || url.ends_with(".tgz")) {
        return Err(anyhow!(
            "Unsupported EMBEDDINGS_MODEL_URL (expected .tar.gz/.tgz): {url}"
        ));
    }

    let stage = staging_dir(model_dir)?;
    tokio::fs::create_dir_all(&stage)
        .await
        .with_context(|| format!("Failed to create {}", stage.display()))?;

    let archive_path = stage.join("model.tar.gz");
    let got_sha = download_to_file(url, &archive_path).await?;
    if let Some(expected) = sha256_hex {
        let expected = expected.trim().to_lowercase();
        if got_sha != expected {
            let _ = tokio::fs::remove_dir_all(&stage).await;
            return Err(anyhow!(
                "EMBEDDINGS_MODEL_SHA256 mismatch (expected {expected}, got {got_sha})"
            ));
        }
        info!(
            sha256 = %got_sha,
            "Verified embeddings model archive checksum"
        );
    }

    let dest = stage.join("model");
    tokio::fs::create_dir_all(&dest)
        .await
        .with_context(|| format!("Failed to create {}", dest.display()))?;

    let archive_path2 = archive_path.clone();
    let dest2 = dest.clone();
    tokio::task::spawn_blocking(move || -> Result<()> { unpack_tar_gz(&archive_path2, &dest2) })
        .await
        .context("Failed to join model unpack task")??;

    tokio::task::spawn_blocking({
        let dest = dest.clone();
        move || maybe_promote_single_subdir(&dest)
    })
    .await
    .context("Failed to join model layout fixup task")??;

    if !has_required_model_files(&dest) {
        let _ = tokio::fs::remove_dir_all(&stage).await;
        return Err(anyhow!(
            "Downloaded model is missing required files (expected config.json, tokenizer.json, and safetensors) in {}",
            dest.display()
        ));
    }

    if model_dir.exists() {
        tokio::fs::remove_dir_all(model_dir)
            .await
            .with_context(|| format!("Failed to remove {}", model_dir.display()))?;
    }
    tokio::fs::rename(&dest, model_dir)
        .await
        .with_context(|| format!("Failed to move model into {}", model_dir.display()))?;

    let _ = tokio::fs::remove_dir_all(&stage).await;
    info!(model_dir = %model_dir.display(), "Embeddings model download complete");
    Ok(true)
}

fn hf_resolve_url(repo: &str, revision: &str, filename: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{revision}/{filename}")
}

#[derive(serde::Deserialize)]
struct SafetensorsIndex {
    weight_map: std::collections::HashMap<String, String>,
}

fn parse_shards_from_index_json(text: &str) -> Result<Vec<String>> {
    let idx: SafetensorsIndex =
        serde_json::from_str(text).context("Failed to parse model.safetensors.index.json")?;
    let mut unique = std::collections::BTreeSet::new();
    for v in idx.weight_map.values() {
        unique.insert(v.to_string());
    }
    if unique.is_empty() {
        return Err(anyhow!("Empty weight_map in model.safetensors.index.json"));
    }
    Ok(unique.into_iter().collect())
}

pub async fn ensure_candle_model_from_huggingface(
    model_dir: &Path,
    repo: &str,
    revision: &str,
    hf_token: Option<&str>,
) -> Result<bool> {
    if has_required_model_files(model_dir) {
        debug!(model_dir = %model_dir.display(), "Embeddings model already present");
        return Ok(false);
    }

    info!(
        model_dir = %model_dir.display(),
        repo = %repo,
        revision = %revision,
        "Downloading embeddings model from Hugging Face"
    );

    let stage = staging_dir(model_dir)?;
    tokio::fs::create_dir_all(&stage)
        .await
        .with_context(|| format!("Failed to create {}", stage.display()))?;

    let dest = stage.join("model");
    tokio::fs::create_dir_all(&dest)
        .await
        .with_context(|| format!("Failed to create {}", dest.display()))?;

    let client = reqwest::Client::builder()
        .default_headers(default_headers(hf_token)?)
        .build()
        .context("Failed to build HTTP client")?;

    let config_url = hf_resolve_url(repo, revision, "config.json");
    let tokenizer_url = hf_resolve_url(repo, revision, "tokenizer.json");
    debug!(url = %config_url, "Downloading config.json");
    download_to_file_with_client(&client, &config_url, &dest.join("config.json")).await?;
    debug!(url = %tokenizer_url, "Downloading tokenizer.json");
    download_to_file_with_client(&client, &tokenizer_url, &dest.join("tokenizer.json")).await?;

    let single_url = hf_resolve_url(repo, revision, "model.safetensors");
    let single_ok = client
        .head(&single_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    if single_ok {
        debug!(url = %single_url, "Downloading model.safetensors");
        download_to_file_with_client(&client, &single_url, &dest.join("model.safetensors")).await?;
    } else {
        warn!("model.safetensors missing; falling back to sharded safetensors");
        let index_url = hf_resolve_url(repo, revision, "model.safetensors.index.json");
        let index_path = dest.join("model.safetensors.index.json");
        debug!(url = %index_url, "Downloading model.safetensors.index.json");
        download_to_file_with_client(&client, &index_url, &index_path).await?;

        let index_text = tokio::fs::read_to_string(&index_path)
            .await
            .with_context(|| format!("Failed to read {}", index_path.display()))?;
        let shards = parse_shards_from_index_json(&index_text)?;
        for shard in shards {
            let url = hf_resolve_url(repo, revision, &shard);
            let path = dest.join(&shard);
            debug!(url = %url, file = %path.display(), "Downloading shard");
            download_to_file_with_client(&client, &url, &path).await?;
        }
    }

    tokio::task::spawn_blocking({
        let dest = dest.clone();
        move || maybe_promote_single_subdir(&dest)
    })
    .await
    .context("Failed to join model layout fixup task")??;

    if !has_required_model_files(&dest) {
        let _ = tokio::fs::remove_dir_all(&stage).await;
        return Err(anyhow!(
            "Downloaded model is missing required files (expected config.json, tokenizer.json, and safetensors) in {}",
            dest.display()
        ));
    }

    if model_dir.exists() {
        tokio::fs::remove_dir_all(model_dir)
            .await
            .with_context(|| format!("Failed to remove {}", model_dir.display()))?;
    }
    tokio::fs::rename(&dest, model_dir)
        .await
        .with_context(|| format!("Failed to move model into {}", model_dir.display()))?;

    let _ = tokio::fs::remove_dir_all(&stage).await;
    info!(model_dir = %model_dir.display(), "Embeddings model download complete");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hf_resolve_url_builds_expected() {
        assert_eq!(
            hf_resolve_url("org/repo", "main", "config.json"),
            "https://huggingface.co/org/repo/resolve/main/config.json"
        );
    }

    #[test]
    fn parse_shards_from_index_json_dedupes() {
        let text = r#"
{
  "metadata": {},
  "weight_map": {
    "a": "model-00001-of-00002.safetensors",
    "b": "model-00002-of-00002.safetensors",
    "c": "model-00001-of-00002.safetensors"
  }
}
"#;
        let shards = parse_shards_from_index_json(text).unwrap();
        assert_eq!(
            shards,
            vec![
                "model-00001-of-00002.safetensors".to_string(),
                "model-00002-of-00002.safetensors".to_string()
            ]
        );
    }
}
