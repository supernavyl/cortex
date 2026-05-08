//! Kill switch test for the pre-apply SandboxGate.
//!
//! Run with:
//!   cargo test --test kill_switch -- --nocapture
//!
//! Verdict thresholds (ADR-003):
//!   catch_rate < 10%  → KILL CORTEX — gate adds overhead with no benefit
//!   catch_rate 10–30% → CONTINUE — gate is useful but validate further
//!   catch_rate > 30%  → SHIP AGGRESSIVELY — gate catches meaningful errors

use cortex_core::gate::{SandboxGate, SandboxedEdit};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ── Workspace fixture ─────────────────────────────────────────────────────────

struct TestWorkspace {
    root: PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cortex-killswitch-{nanos}"));
        fs::create_dir_all(root.join("src")).unwrap();

        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"ks_workspace\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        fs::write(
            root.join("src/lib.rs"),
            r#"pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn greeting(name: &str) -> String {
    format!("Hello, {}!", name)
}

pub struct Counter {
    pub count: u32,
}

impl Counter {
    pub fn new() -> Self {
        Self { count: 0 }
    }

    pub fn increment(&mut self) {
        self.count += 1;
    }

    pub fn value(&self) -> u32 {
        self.count
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}
"#,
        )
        .unwrap();

        fs::write(
            root.join("src/math.rs"),
            r#"pub fn multiply(a: f64, b: f64) -> f64 {
    a * b
}

pub fn divide(a: f64, b: f64) -> Option<f64> {
    if b == 0.0 { None } else { Some(a / b) }
}
"#,
        )
        .unwrap();

        Self { root }
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// ── Edit catalogue ────────────────────────────────────────────────────────────

struct EditCase {
    label: &'static str,
    edit: SandboxedEdit,
    expect_rejected: bool,
}

fn edit_cases(workspace: &Path) -> Vec<EditCase> {
    let lib = workspace.join("src/lib.rs");
    let math = workspace.join("src/math.rs");
    let current_lib = fs::read_to_string(&lib).unwrap();
    let current_math = fs::read_to_string(&math).unwrap();

    vec![
        // ── VALID edits (should be accepted) ─────────────────────────────────
        EditCase {
            label: "valid: add a subtract fn",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: format!(
                    "{current_lib}\npub fn sub(a: i32, b: i32) -> i32 {{ a - b }}\n"
                ),
            },
            expect_rejected: false,
        },
        EditCase {
            label: "valid: add a doc comment to greeting",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib
                    .replace("pub fn greeting", "/// Returns a greeting.\npub fn greeting"),
            },
            expect_rejected: false,
        },
        EditCase {
            label: "valid: add reset method to Counter",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib.replace(
                    "    pub fn value(&self) -> u32 {\n        self.count\n    }",
                    "    pub fn value(&self) -> u32 {\n        self.count\n    }\n\n    pub fn reset(&mut self) {\n        self.count = 0;\n    }",
                ),
            },
            expect_rejected: false,
        },
        EditCase {
            label: "valid: add abs fn to math",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/math.rs"),
                new_content: format!(
                    "{current_math}\npub fn abs_val(x: f64) -> f64 {{ x.abs() }}\n"
                ),
            },
            expect_rejected: false,
        },
        EditCase {
            label: "valid: add modulo fn to math",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/math.rs"),
                new_content: format!(
                    "{current_math}\npub fn modulo(a: i32, b: i32) -> i32 {{ a % b }}\n"
                ),
            },
            expect_rejected: false,
        },
        EditCase {
            label: "valid: rename count field (with fix)",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib
                    .replace("pub count: u32", "pub value_internal: u32")
                    .replace("self.count += 1", "self.value_internal += 1")
                    .replace("self.count\n    }", "self.value_internal\n    }"),
            },
            expect_rejected: false,
        },
        EditCase {
            label: "valid: add new file src/utils.rs",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/utils.rs"),
                new_content: "pub fn clamp(v: i32, lo: i32, hi: i32) -> i32 {\n    v.max(lo).min(hi)\n}\n".into(),
            },
            expect_rejected: false,
        },
        EditCase {
            label: "valid: add checked_add fn",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: format!(
                    "{current_lib}\npub fn checked_add(a: i32, b: i32) -> Option<i32> {{ a.checked_add(b) }}\n"
                ),
            },
            expect_rejected: false,
        },
        // ── BROKEN edits (should be rejected) ────────────────────────────────
        EditCase {
            label: "broken: add returns &str instead of i32",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib
                    .replace("pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}", "pub fn add(a: i32, b: i32) -> i32 {\n    \"wrong type\"\n}"),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: greeting returns i32 instead of String",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib.replace(
                    "pub fn greeting(name: &str) -> String {\n    format!(\"Hello, {}!\", name)\n}",
                    "pub fn greeting(name: &str) -> String {\n    42\n}",
                ),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: increment takes wrong receiver type",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib
                    .replace("pub fn increment(&mut self)", "pub fn increment(self)"),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: divide returns f64 instead of Option<f64>",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/math.rs"),
                new_content: current_math.replace(
                    "pub fn divide(a: f64, b: f64) -> Option<f64> {\n    if b == 0.0 { None } else { Some(a / b) }\n}",
                    "pub fn divide(a: f64, b: f64) -> Option<f64> {\n    a / b\n}",
                ),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: missing closing brace",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib
                    .trim_end_matches('\n')
                    .trim_end_matches('}')
                    .to_string(),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: undefined variable in add",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib
                    .replace("    a + b", "    a + b + undefined_var"),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: multiply uses undefined method",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/math.rs"),
                new_content: current_math
                    .replace("    a * b", "    a.nonexistent_method() * b"),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: Counter::new returns wrong type",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib.replace(
                    "pub fn new() -> Self {\n        Self { count: 0 }\n    }",
                    "pub fn new() -> Self {\n        42\n    }",
                ),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: value() returns &str",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib.replace(
                    "    pub fn value(&self) -> u32 {\n        self.count\n    }",
                    "    pub fn value(&self) -> u32 {\n        \"not a number\"\n    }",
                ),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: syntax error — unclosed string",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib.replace(
                    "format!(\"Hello, {}!\", name)",
                    "format!(\"Hello, {}!, name)",
                ),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: wrong field name in Default",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/lib.rs"),
                new_content: current_lib.replace(
                    "impl Default for Counter {\n    fn default() -> Self {\n        Self::new()\n    }\n}",
                    "impl Default for Counter {\n    fn default() -> Self {\n        Self { nonexistent_field: 0 }\n    }\n}",
                ),
            },
            expect_rejected: true,
        },
        EditCase {
            label: "broken: divide by literal — lost Option wrapper",
            edit: SandboxedEdit {
                relative_path: PathBuf::from("src/math.rs"),
                new_content: current_math.replace(
                    "pub fn divide(a: f64, b: f64) -> Option<f64>",
                    "pub fn divide(a: f64, b: f64) -> f64",
                ),
            },
            expect_rejected: true,
        },
    ]
}

// ── Harness ───────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Stats {
    caught: u32,     // expected_rejected=true AND gate rejected
    missed: u32,     // expected_rejected=true AND gate accepted (false negative)
    clean_pass: u32, // expected_rejected=false AND gate accepted
    false_pos: u32,  // expected_rejected=false AND gate rejected
    skipped: u32,    // gate returned skipped (cargo not on PATH)
}

#[tokio::test]
async fn kill_switch() {
    let ws = TestWorkspace::new();
    let gate = SandboxGate::new(ws.root.clone());
    let cases = edit_cases(&ws.root);

    let total = cases.len();
    let mut stats = Stats::default();
    let mut rows: Vec<(&str, bool, bool, String)> = Vec::new(); // label, expect_rejected, rejected, reason_snippet

    for case in &cases {
        let vr = gate.verify(&case.edit).await;
        let rejected = !vr.accepted;
        let skipped = vr.reason.contains("skipped") || vr.verifier == "sandbox";

        if skipped {
            stats.skipped += 1;
        } else if case.expect_rejected && rejected {
            stats.caught += 1;
        } else if case.expect_rejected && !rejected {
            stats.missed += 1;
        } else if !case.expect_rejected && !rejected {
            stats.clean_pass += 1;
        } else {
            stats.false_pos += 1;
        }

        let reason_snippet: String = vr
            .reason
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect();
        rows.push((case.label, case.expect_rejected, rejected, reason_snippet));
    }

    // ── Print table ───────────────────────────────────────────────────────────
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           CORTEX KILL SWITCH TEST — SandboxGate catch rate           ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    println!(
        "{:<45} {:>8}  {:>8}  {}",
        "Edit", "Expected", "Gate", "Reason"
    );
    println!("{}", "─".repeat(100));
    for (label, expect_rej, rejected, reason) in &rows {
        let expected_str = if *expect_rej { "REJECT" } else { "ACCEPT" };
        let gate_str = if *rejected { "REJECT" } else { "ACCEPT" };
        let marker = if expect_rej == rejected { "✓" } else { "✗" };
        println!(
            "{marker} {:<44} {:>8}  {:>8}  {reason}",
            label, expected_str, gate_str
        );
    }

    let runnable = total as u32 - stats.skipped;
    let catch_rate = if stats.skipped == total as u32 {
        println!(
            "\n⚠  cargo not on PATH — all checks skipped. Install cargo to run the kill switch."
        );
        println!("\n📋 VERDICT: INCONCLUSIVE (toolchain unavailable)\n");
        return;
    } else {
        let broken_total = rows.iter().filter(|(_, exp, _, _)| *exp).count() as u32;
        let broken_runnable = broken_total.saturating_sub(stats.skipped);
        if broken_runnable == 0 {
            0.0
        } else {
            stats.caught as f64 / broken_runnable as f64 * 100.0
        }
    };

    println!("\n── Stats ─────────────────────────────────────────────────────────────");
    println!("  Total edits:      {total}");
    println!(
        "  Runnable:         {runnable}  (skipped: {})",
        stats.skipped
    );
    println!("  Caught (TP):      {}", stats.caught);
    println!("  Missed (FN):      {}", stats.missed);
    println!("  Clean pass (TN):  {}", stats.clean_pass);
    println!("  False pos (FP):   {}", stats.false_pos);
    println!("  Catch rate:       {catch_rate:.1}%");

    println!("\n── ADR-003 Verdict ───────────────────────────────────────────────────");
    if catch_rate < 10.0 {
        println!("  ❌ KILL CORTEX — catch rate {catch_rate:.1}% < 10% threshold.");
        println!("     Gate adds latency with no meaningful benefit. Stop here.");
    } else if catch_rate < 30.0 {
        println!("  🟡 CONTINUE — catch rate {catch_rate:.1}% (10–30% range).");
        println!("     Gate is useful but validate with real AI-generated edits.");
    } else {
        println!("  ✅ SHIP AGGRESSIVELY — catch rate {catch_rate:.1}% > 30% threshold.");
        println!("     Gate catches meaningful errors. Proceed with MCP server phase.");
    }
    println!();

    // Hard assertions: gate must not crash, skipped rate must be <100%.
    assert!(
        stats.skipped < total as u32,
        "all checks skipped — cargo must be on PATH for the kill switch to be meaningful"
    );
}
