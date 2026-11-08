use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use unity_extractor::{extract_unitypackage, ExtractOptions, QueueItem};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: batch_extract <dossier_ou_fichier.unitypackage> [sortie_parent]");
        std::process::exit(1);
    }

    let input = PathBuf::from(&args[0]);
    let out_root = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| input.join("extracted"));

    let packages = collect_packages(&input);
    if packages.is_empty() {
        eprintln!("Aucun .unitypackage trouvé dans {}", input.display());
        std::process::exit(2);
    }

    println!("Packages trouvés: {}", packages.len());
    let _ = fs::create_dir_all(&out_root);

    let options = ExtractOptions {
        include_meta: true,
        include_preview: false,
    };

    let mut failed = Vec::new();

    for (i, package) in packages.iter().enumerate() {
        let item = QueueItem::from_package(package.clone(), Some(out_root.clone()));
        let output_dir = item.output_dir();

        println!(
            "\n=== [{}/{}] {} ===",
            i + 1,
            packages.len(),
            package.display()
        );
        println!(
            "Dossier: {} → {}",
            item.folder_name,
            output_dir.display()
        );

        let started = Instant::now();
        let mut last_pct = -1i32;

        let result = extract_unitypackage(package, &output_dir, &options, |fraction, message| {
            let pct = (fraction * 100.0) as i32;
            if pct != last_pct && (pct % 5 == 0 || fraction >= 1.0) {
                println!("  [{pct:3}%] {message}");
                last_pct = pct;
            }
        });

        match result {
            Ok(report) => {
                println!(
                    "OK — {} extrait(s), {} ignoré(s) en {:.1}s",
                    report.extracted,
                    report.skipped,
                    started.elapsed().as_secs_f32()
                );
                if !report.errors.is_empty() {
                    println!("  Avertissements: {}", report.errors.len());
                    for err in report.errors.iter().take(10) {
                        println!("   - {err}");
                    }
                }
            }
            Err(err) => {
                eprintln!("ÉCHEC: {err:#}");
                failed.push((package.clone(), err.to_string()));
            }
        }
    }

    println!("\n========== RÉSUMÉ ==========");
    if failed.is_empty() {
        println!("Tous les packages ont été extraits.");
        println!("Dossier: {}", out_root.display());
    } else {
        println!("{} échec(s):", failed.len());
        for (path, err) in &failed {
            println!(" - {}: {}", path.display(), err);
        }
        std::process::exit(3);
    }
}

fn collect_packages(input: &Path) -> Vec<PathBuf> {
    if input.is_file() {
        return vec![input.to_path_buf()];
    }

    let mut packages = Vec::new();
    if let Ok(entries) = fs::read_dir(input) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("unitypackage"))
                == Some(true)
            {
                packages.push(path);
            }
        }
    }
    packages.sort();
    packages
}
