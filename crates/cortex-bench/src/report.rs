//! Terminal report printer.

use crate::metrics::{BenchResult, ModelSummary};

/// Print the full benchmark report to stdout.
pub fn print_report(results: &[BenchResult], summaries: &[ModelSummary]) {
    if summaries.is_empty() {
        println!("No results to report.");
        return;
    }

    // Sort: success_rate DESC, then tok_per_sec DESC.
    let mut sorted: Vec<&ModelSummary> = summaries.iter().collect();
    sorted.sort_by(|a, b| {
        b.success_rate
            .partial_cmp(&a.success_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.avg_tok_per_sec
                    .partial_cmp(&a.avg_tok_per_sec)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    // Column widths.
    let model_col = sorted
        .iter()
        .map(|s| s.model.len())
        .max()
        .unwrap_or(10)
        .max(25);

    let header_width = model_col + 2 + 8 + 2 + 10 + 2 + 9 + 2 + 13 + 2;

    // ── Header ────────────────────────────────────────────────────────────────
    println!("╔{}╗", "═".repeat(header_width));
    println!(
        "║  {:<width$}                                              ║",
        "CORTEX MODEL BENCHMARK",
        width = model_col
    );
    println!(
        "╠{:═<col$}╦{:═<8}╦{:═<10}╦{:═<9}╦{:═<13}╣",
        "═".repeat(model_col + 2),
        "",
        "",
        "",
        "",
        col = 0
    );
    println!(
        "║  {:<model_col$}  ║ {:<6} ║ {:<8} ║ {:<7} ║ {:<11} ║",
        "Model",
        "Pass",
        "Avg ms",
        "tok/s",
        "Avg rounds",
        model_col = model_col
    );
    println!(
        "╠{:═<col$}╬{:═<8}╬{:═<10}╬{:═<9}╬{:═<13}╣",
        "═".repeat(model_col + 2),
        "",
        "",
        "",
        "",
        col = 0
    );

    // ── Rows ──────────────────────────────────────────────────────────────────
    for s in &sorted {
        let successes = (s.success_rate * s.tasks_run as f32).round() as usize;
        let pass = format!("{successes}/{}", s.tasks_run);
        println!(
            "║  {:<model_col$}  ║ {:<6} ║ {:<8.0} ║ {:<7.1} ║ {:<11.1} ║",
            s.model,
            pass,
            s.avg_latency_ms,
            s.avg_tok_per_sec,
            s.avg_rounds,
            model_col = model_col
        );
    }

    println!(
        "╚{:═<col$}╩{:═<8}╩{:═<10}╩{:═<9}╩{:═<13}╝",
        "═".repeat(model_col + 2),
        "",
        "",
        "",
        "",
        col = 0
    );

    // ── Recommendations ───────────────────────────────────────────────────────
    println!();
    println!("── RECOMMENDATIONS ─────────────────────────────────────────────────────────");

    let best_local = sorted.iter().find(|s| !s.is_cloud);
    let best_cloud = sorted.iter().find(|s| s.is_cloud);

    if let Some(s) = best_local {
        println!(
            "  Best local:      {} ({:.0} tok/s, {:.0}% pass)",
            s.model,
            s.avg_tok_per_sec,
            s.success_rate * 100.0
        );
    }
    if let Some(s) = best_cloud {
        println!(
            "  Best cloud:      {} ({:.0} tok/s, {:.0}% pass)",
            s.model,
            s.avg_tok_per_sec,
            s.success_rate * 100.0
        );
    }

    // Best overall = top of the sorted list (highest success rate).
    if let Some(s) = sorted.first() {
        println!("  Best overall:    {}", s.model);
    }

    // Fastest = lowest avg_latency_ms.
    if let Some(fastest) = summaries.iter().min_by(|a, b| {
        a.avg_latency_ms
            .partial_cmp(&b.avg_latency_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        println!(
            "  Fastest:         {} (avg {:.0}ms)",
            fastest.model, fastest.avg_latency_ms
        );
    }

    // Most reliable = lowest avg_rounds.
    if let Some(reliable) = summaries.iter().filter(|s| s.tasks_run > 0).min_by(|a, b| {
        a.avg_rounds
            .partial_cmp(&b.avg_rounds)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        println!(
            "  Most reliable:   {} ({:.1} avg rounds)",
            reliable.model, reliable.avg_rounds
        );
    }

    let has_cloud = summaries.iter().any(|s| s.is_cloud);
    let has_local = summaries.iter().any(|s| !s.is_cloud);
    let run_together = match (has_local, has_cloud) {
        (true, true) => {
            "YES — cloud models run in parallel with no conflict.\n                   \
             Local models serialize on GPU; run cloud + local simultaneously."
        }
        (true, false) => "LOCAL ONLY — all models share GPU; sequential execution recommended.",
        (false, true) => "CLOUD ONLY — all models run in parallel.",
        (false, false) => "N/A",
    };
    println!("  Run together?:   {run_together}");
    println!("─────────────────────────────────────────────────────────────────────────────");

    // ── Per-task detail ───────────────────────────────────────────────────────
    println!();
    println!("── TASK DETAIL ──────────────────────────────────────────────────────────────");
    // Header
    println!(
        "  {:<20}  {:<model_col$}  {:<6}  {:<8}  {:<7}  {:<6}  {:<8}  {:<5}",
        "Task",
        "Model",
        "OK",
        "ms",
        "tok/s",
        "lines",
        "tokens",
        "rnds",
        model_col = model_col
    );
    println!(
        "  {}",
        "─".repeat(20 + 2 + model_col + 2 + 6 + 2 + 8 + 2 + 7 + 2 + 6 + 2 + 8 + 2 + 5)
    );

    for r in results {
        println!(
            "  {:<20}  {:<model_col$}  {:<6}  {:<8}  {:<7.1}  {:<6}  {:<8}  {:<5}",
            r.task,
            r.model,
            if r.success { "PASS" } else { "FAIL" },
            r.latency_ms,
            r.tok_per_sec,
            r.lines_written,
            r.total_tokens(),
            r.rounds,
            model_col = model_col
        );
        if let Some(ref err) = r.error {
            let short = err
                .lines()
                .next()
                .unwrap_or(err)
                .chars()
                .take(80)
                .collect::<String>();
            println!("    error: {short}");
        }
    }
    println!("─────────────────────────────────────────────────────────────────────────────");
}
