use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{copy, BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use tar::Archive;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractOptions {
    pub include_meta: bool,
    pub include_preview: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractReport {
    pub extracted: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    pub output_dir: PathBuf,
}

/// Élément de file d'attente d'extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub package_path: PathBuf,
    /// Dossier parent où créer le dossier extrait.
    pub output_parent: PathBuf,
    /// Nom du dossier de sortie (défaut = nom du .unitypackage sans extension).
    pub folder_name: String,
}

impl QueueItem {
    pub fn from_package(package_path: PathBuf, output_parent: Option<PathBuf>) -> Self {
        let folder_name = package_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "extracted".into());
        let output_parent = output_parent.unwrap_or_else(|| {
            package_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        });
        Self {
            package_path,
            output_parent,
            folder_name,
        }
    }

    pub fn output_dir(&self) -> PathBuf {
        resolve_output_dir(&self.output_parent, &self.folder_name)
    }
}

/// Construit le chemin final de sortie à partir du parent + nom de dossier.
pub fn resolve_output_dir(output_parent: &Path, folder_name: &str) -> PathBuf {
    let name = sanitize_folder_name(folder_name);
    output_parent.join(name)
}

fn sanitize_folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.');
    if cleaned.is_empty() {
        "extracted".into()
    } else {
        cleaned.to_string()
    }
}

#[derive(Default)]
struct AssetEntry {
    pathname: Option<String>,
    asset_path: Option<PathBuf>,
    meta_path: Option<PathBuf>,
    preview_path: Option<PathBuf>,
}

pub fn extract_unitypackage(
    package_path: &Path,
    output_dir: &Path,
    options: &ExtractOptions,
    mut on_progress: impl FnMut(f32, String),
) -> Result<ExtractReport> {
    if !package_path.exists() {
        bail!("Le fichier .unitypackage est introuvable.");
    }

    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Impossible de créer le dossier de sortie: {}",
            output_dir.display()
        )
    })?;

    let staging_dir = output_dir.join(".unity_extractor_staging");
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir).ok();
    }
    fs::create_dir_all(&staging_dir)?;

    let extract_result = (|| {
        on_progress(0.02, "Lecture du package…".into());

        let file = File::open(package_path)
            .with_context(|| format!("Impossible d'ouvrir {}", package_path.display()))?;
        let mut buffered = BufReader::new(file);
        buffered
            .fill_buf()
            .context("Impossible de lire l’en-tête du package")?;
        let is_gzip = {
            let buf = buffered.buffer();
            buf.len() >= 2 && buf[0] == 0x1f && buf[1] == 0x8b
        };

        let entries = if is_gzip {
            on_progress(0.03, "Format détecté: tar.gz".into());
            index_archive(
                Archive::new(GzDecoder::new(buffered)),
                &staging_dir,
                &mut on_progress,
            )?
        } else {
            // Certains .unitypackage (ex: packs UI) sont un tar brut non gzippé
            on_progress(0.03, "Format détecté: tar (non compressé)".into());
            index_archive(Archive::new(buffered), &staging_dir, &mut on_progress)?
        };

        let total_assets = entries.len().max(1) as f32;
        let mut report = ExtractReport {
            output_dir: output_dir.to_path_buf(),
            ..Default::default()
        };

        for (index, (guid, entry)) in entries.into_iter().enumerate() {
            let progress = 0.45 + (index as f32 / total_assets) * 0.50;
            let Some(pathname) = entry.pathname.filter(|p| !p.is_empty()) else {
                report.skipped += 1;
                report
                    .errors
                    .push(format!("GUID {guid}: pathname manquant, entrée ignorée"));
                continue;
            };

            on_progress(progress, format!("Extraction: {pathname}"));

            let relative = sanitize_relative_path(&pathname);
            if relative.as_os_str().is_empty() {
                report.skipped += 1;
                report
                    .errors
                    .push(format!("GUID {guid}: chemin invalide ({pathname})"));
                continue;
            }

            let dest = long_path(&output_dir.join(&relative));

            if let Some(parent) = dest.parent() {
                if let Err(err) = fs::create_dir_all(parent) {
                    report.skipped += 1;
                    report.errors.push(format!(
                        "{pathname}: impossible de créer le dossier ({err})"
                    ));
                    continue;
                }
            }

            if let Some(asset_src) = entry.asset_path {
                match move_or_copy(&asset_src, &dest) {
                    Ok(()) => report.extracted += 1,
                    Err(err) => {
                        report.skipped += 1;
                        report
                            .errors
                            .push(format!("{pathname}: écriture asset échouée ({err})"));
                    }
                }
            } else if dest.extension().is_none() {
                let _ = fs::create_dir_all(&dest);
            } else {
                report.skipped += 1;
            }

            if options.include_meta {
                if let Some(meta_src) = entry.meta_path {
                    let meta_path = PathBuf::from(format!("{}.meta", dest.display()));
                    let _ = move_or_copy(&meta_src, &meta_path);
                }
            }

            if options.include_preview {
                if let Some(preview_src) = entry.preview_path {
                    let preview_path = PathBuf::from(format!("{}.preview.png", dest.display()));
                    let _ = move_or_copy(&preview_src, &preview_path);
                }
            }
        }

        on_progress(1.0, "Extraction terminée".into());
        Ok(report)
    })();

    // Always clean staging, even on failure
    let _ = fs::remove_dir_all(&staging_dir);
    extract_result
}

fn index_archive<R: Read>(
    mut archive: Archive<R>,
    staging_dir: &Path,
    on_progress: &mut impl FnMut(f32, String),
) -> Result<HashMap<String, AssetEntry>> {
    let mut entries: HashMap<String, AssetEntry> = HashMap::new();
    let mut total_members = 0usize;

    for entry in archive.entries().context("Archive tar invalide")? {
        let mut entry = match entry {
            Ok(e) => e,
            Err(err) => {
                eprintln!("Entrée tar ignorée: {err}");
                continue;
            }
        };

        let path = match entry.path() {
            Ok(p) => p.into_owned(),
            Err(_) => continue,
        };

        let parts: Vec<_> = path
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();

        if parts.len() < 2 {
            continue;
        }

        let guid = parts[0].clone();
        let kind = parts[1].as_str();
        let guid_dir = staging_dir.join(&guid);
        fs::create_dir_all(&guid_dir)?;
        let slot = entries.entry(guid).or_default();

        match kind {
            "pathname" => {
                let mut raw = Vec::new();
                entry.read_to_end(&mut raw)?;
                if let Some(cleaned) = decode_pathname(&raw) {
                    slot.pathname = Some(cleaned);
                }
            }
            "asset" => {
                let dest = guid_dir.join("asset");
                write_entry_to_file(&mut entry, &dest)?;
                slot.asset_path = Some(dest);
            }
            "asset.meta" => {
                let dest = guid_dir.join("asset.meta");
                write_entry_to_file(&mut entry, &dest)?;
                slot.meta_path = Some(dest);
            }
            "preview.png" => {
                let dest = guid_dir.join("preview.png");
                write_entry_to_file(&mut entry, &dest)?;
                slot.preview_path = Some(dest);
            }
            _ => {}
        }

        total_members += 1;
        if total_members % 50 == 0 {
            on_progress(
                0.05 + (total_members as f32 * 0.0005).min(0.40),
                format!("Indexation… {total_members} entrées"),
            );
        }
    }

    Ok(entries)
}

fn write_entry_to_file(entry: &mut tar::Entry<impl Read>, dest: &Path) -> Result<()> {
    let mut out = File::create(dest)?;
    copy(entry, &mut out)?;
    out.flush()?;
    Ok(())
}

fn move_or_copy(src: &Path, dest: &Path) -> Result<()> {
    let dest = long_path(dest);
    let src = long_path(src);

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    if dest.exists() {
        fs::remove_file(&dest).ok();
    }

    match fs::rename(&src, &dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(&src, &dest)?;
            let _ = fs::remove_file(&src);
            Ok(())
        }
    }
}

/// Préfixe Windows `\\?\` pour supporter les chemins > 260 caractères.
fn long_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if s.starts_with(r"\\?\") || s.starts_with("//?/") {
            return path.to_path_buf();
        }
        if let Ok(abs) = path.canonicalize().or_else(|_| {
            // canonicalize fails if path doesn't exist yet; build absolute manually
            if path.is_absolute() {
                Ok(path.to_path_buf())
            } else {
                std::env::current_dir().map(|cwd| cwd.join(path))
            }
        }) {
            let abs_str = abs.to_string_lossy().replace('/', "\\");
            if abs_str.starts_with(r"\\?\") {
                return PathBuf::from(abs_str);
            }
            if abs_str.starts_with(r"\\") {
                // UNC path
                return PathBuf::from(format!(r"\\?\UNC\{}", abs_str.trim_start_matches(r"\\")));
            }
            return PathBuf::from(format!(r"\\?\{abs_str}"));
        }
    }
    path.to_path_buf()
}

fn decode_pathname(raw: &[u8]) -> Option<String> {
    let null_count = raw.iter().filter(|b| **b == 0).count();
    // Beaucoup de NUL → probablement UTF-16 LE (chemin ASCII Unity classique)
    if raw.len() >= 4 && null_count >= raw.len() / 4 {
        let shorts: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|c| *c != 0)
            .collect();
        if let Ok(s) = String::from_utf16(&shorts) {
            let cleaned = normalize_pathname(&s);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }

    let text = String::from_utf8_lossy(raw);
    let cleaned = normalize_pathname(&text);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn normalize_pathname(text: &str) -> String {
    // Certains packages (ex: vieux packs Synty) appendent du bruit après un \n ("\n00")
    let first_line = text.lines().next().unwrap_or(text);
    first_line
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .trim_matches('\u{feff}')
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

fn sanitize_relative_path(raw: &str) -> PathBuf {
    let cleaned = raw.trim().trim_start_matches(['/', '\\']);
    let mut out = PathBuf::new();

    for component in Path::new(cleaned).components() {
        match component {
            Component::Normal(part) => {
                let name = part.to_string_lossy();
                // Refuse Windows-illegal characters in a single component
                if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0')) {
                    continue;
                }
                out.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }

    out
}
