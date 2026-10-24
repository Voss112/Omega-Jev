//! Whisper model file: downloaded once from Hugging Face (ggerganov/whisper.cpp), SHA-256 checked before use.

use std::{fs, io::Write as _, path::PathBuf};

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt;
use reqwest::Client;
use ring::digest;

use crate::infra::paths;

pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    /// Lower-case hex, from the repository's Git LFS pointer.
    pub sha256: &'static str,
    pub size: u64,
}

/// Whisper large-v3-turbo, 5-bit: near-large-v3 accuracy in 99 languages, 4 decoder layers (fast decoding).
pub const TURBO: ModelFile = ModelFile {
    name: "ggml-large-v3-turbo-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
    sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    size: 574_041_195,
};

pub fn path(model: &ModelFile) -> PathBuf {
    paths::models().join(model.name)
}

/// A complete file is on disk (its hash was checked when it was downloaded).
pub fn installed(model: &ModelFile) -> bool {
    fs::metadata(path(model)).is_ok_and(|m| m.len() == model.size)
}

/// The model path, downloading it first if needed. `progress(done, total)` is called as bytes arrive.
pub async fn ensure(http: &Client, model: &ModelFile, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    let dest = path(model);
    if installed(model) {
        return Ok(dest);
    }
    fs::create_dir_all(paths::models()).context("dossier des modèles")?;
    let part = dest.with_extension("part");
    let res = http.get(model.url).send().await.and_then(reqwest::Response::error_for_status).context("téléchargement du modèle")?;
    let mut file = fs::File::create(&part).with_context(|| format!("{}", part.display()))?;
    let mut hash = digest::Context::new(&digest::SHA256);
    let mut done = 0u64;
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("téléchargement interrompu")?;
        done += chunk.len() as u64;
        if done > model.size {
            drop(file);
            let _ = fs::remove_file(&part);
            bail!("modèle plus gros que prévu ({done} octets)");
        }
        hash.update(&chunk);
        file.write_all(&chunk).context("écriture du modèle")?;
        progress(done, model.size);
    }
    file.sync_all().context("écriture du modèle")?;
    drop(file);
    let got = hex(hash.finish().as_ref());
    if done != model.size || got != model.sha256 {
        let _ = fs::remove_file(&part);
        bail!("modèle corrompu ({done} octets, SHA-256 {got}) : supprimé, réessayez");
    }
    fs::rename(&part, &dest).context("installation du modèle")?;
    Ok(dest)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_lower_hex() {
        assert_eq!(hex(digest::digest(&digest::SHA256, b"abc").as_ref()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(TURBO.url.starts_with("https://") && TURBO.url.ends_with(TURBO.name));
    }
}
