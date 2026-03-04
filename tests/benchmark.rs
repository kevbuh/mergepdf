use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

fn collect_test_pdfs() -> (PathBuf, Vec<PathBuf>) {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_data = project_root.join("test_data");
    assert!(test_data.is_dir(), "test_data directory not found");

    let mut pdfs: Vec<PathBuf> = std::fs::read_dir(&test_data)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                && !["merged", "bench_output", "test_output"]
                    .contains(&p.file_stem().unwrap().to_str().unwrap())
        })
        .collect();
    pdfs.sort();
    assert!(!pdfs.is_empty(), "No PDF files found in test_data");
    (test_data, pdfs)
}

#[test]
fn bench_merge_pdfunite() {
    let (test_data, pdfs) = collect_test_pdfs();
    let output = test_data.join("bench_pdfunite.pdf");
    let _ = std::fs::remove_file(&output);

    let mut args: Vec<String> = pdfs.iter().map(|f| f.to_string_lossy().to_string()).collect();
    args.push(output.to_string_lossy().to_string());

    println!("\n[pdfunite] Merging {} PDFs", pdfs.len());

    let start = Instant::now();
    let result = Command::new("pdfunite")
        .args(&args)
        .output()
        .expect("pdfunite not found");
    let elapsed = start.elapsed();

    assert!(
        result.status.success(),
        "pdfunite failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let output_size = std::fs::metadata(&output).unwrap().len();
    println!("  Time:   {:.2}s", elapsed.as_secs_f64());
    println!("  Output: {:.2} MB", output_size as f64 / 1_048_576.0);

    let _ = std::fs::remove_file(&output);
}

#[test]
fn bench_merge_gs() {
    let (test_data, pdfs) = collect_test_pdfs();
    let output = test_data.join("bench_gs.pdf");
    let _ = std::fs::remove_file(&output);

    let mut args = vec![
        "-dBATCH".to_string(),
        "-dNOPAUSE".to_string(),
        "-q".to_string(),
        "-sDEVICE=pdfwrite".to_string(),
        format!("-sOutputFile={}", output.display()),
    ];
    for pdf in &pdfs {
        args.push(pdf.to_string_lossy().to_string());
    }

    println!("\n[gs] Merging {} PDFs", pdfs.len());

    let start = Instant::now();
    let result = Command::new("gs").args(&args).output().expect("gs not found");
    let elapsed = start.elapsed();

    assert!(
        result.status.success(),
        "gs failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let output_size = std::fs::metadata(&output).unwrap().len();
    println!("  Time:   {:.2}s", elapsed.as_secs_f64());
    println!("  Output: {:.2} MB", output_size as f64 / 1_048_576.0);

    let _ = std::fs::remove_file(&output);
}
