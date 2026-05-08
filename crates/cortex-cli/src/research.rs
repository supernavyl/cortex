use anyhow::Result;
use cortex_core::config::Config;
use cortex_core::protocol::{Method, ModelTier, Request, ResponseChunk};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const SCOUT_PROMPT: &str = concat!(
    "You are SCOUT — a raw intelligence miner. Your job: find sources and extract claims with citations.\n\n",
    "STRICT RULES:\n",
    "1. Fetch EXACTLY 3 URLs using bash: `curl -sL --max-time 10 \"<url>\" | head -c 8000`\n",
    "2. After the 3rd fetch, STOP using tools. Write the report immediately.\n",
    "3. Do NOT fetch more than 3 URLs under any circumstances.\n\n",
    "OUTPUT FORMAT:\n",
    "## SCOUT — Source Intelligence Report\n\n",
    "### Sources Found\n",
    "1. [URL] — [HIGH/MED/LOW authority] — [date] — [one sentence]\n\n",
    "### Claims Extracted\n",
    "- [Claim] — SOURCE: [url] — TAG: [DIRECT_QUOTE/PARAPHRASE/INFERRED]\n\n",
    "### Conflicts Detected\n",
    "- [Claim A from Source X] vs [Claim B from Source Y] — UNRESOLVED\n\n",
    "### Coverage Gaps\n",
    "- [What no reliable source covers]\n\n",
    "TOPIC: "
);

const ORACLE_PROMPT: &str = concat!(
    "You are ORACLE — a pattern recognition agent. Apply cross-domain knowledge and historical precedent.\n",
    "Do NOT search the web. Draw from deep reasoning about the topic.\n\n",
    "OUTPUT FORMAT:\n",
    "## ORACLE — Pattern Analysis\n\n",
    "### Historical Context\n",
    "[Prior art, analogues, how this was solved before]\n\n",
    "### Cross-Domain Patterns\n",
    "[What other fields solved analogous problems, and how]\n\n",
    "### The Core Tension\n",
    "[The fundamental trade-off or conflict]\n\n",
    "### Prediction\n",
    "[What the evidence suggests, with confidence %]\n\n",
    "### My Blind Spots\n",
    "[Where my analysis might be wrong]\n\n",
    "TOPIC: "
);

const PHANTOM_HEADER: &str = concat!(
    "You are PHANTOM — an adversarial claims auditor. Find weak evidence, challenge consensus, expose gaps.\n\n",
    "## PHASE 1 INTELLIGENCE:\n\n"
);

const PHANTOM_FOOTER: &str = concat!(
    "\n\n## YOUR TASK: Challenge SCOUT and ORACLE. Find everything wrong.\n\n",
    "OUTPUT FORMAT:\n",
    "## PHANTOM — Verification Report\n\n",
    "### Claims Verified\n",
    "- [Claim] — CONFIRMED: [why]\n\n",
    "### Claims Challenged\n",
    "- [Claim] — FRAGILE: [why]\n\n",
    "### ORACLE Critique\n",
    "- [Specific challenge to ORACLE's analysis]\n\n",
    "### Unstated Assumptions\n",
    "- [Assumption without evidence]\n\n",
    "### The Counterargument Nobody Raised\n",
    "[Strongest case AGAINST emerging consensus]\n\n",
    "### Missing Questions\n",
    "- [Important question not asked]\n\n",
    "### Confidence: [X]%"
);

const VERDICT_HEADER: &str = concat!(
    "You are VERDICT — the final arbiter. Produce a definitive, confidence-rated intelligence brief.\n\n",
    "Rules:\n",
    "1. Tag every major claim: FACT / ESTIMATE / OPINION / UNKNOWN\n",
    "2. Confidence % on every claim\n",
    "3. Resolve every conflict from SCOUT and PHANTOM\n",
    "4. State minimum reliable knowledge (>80% confidence)\n",
    "5. Actionable recommendations\n",
    "6. Do not hedge — take a position\n\n",
    "## ALL RESEARCH INTELLIGENCE:\n\n"
);

const VERDICT_FOOTER: &str = concat!(
    "\n\n## OUTPUT FORMAT:\n",
    "## VERDICT — Intelligence Brief\n\n",
    "### Minimum Reliable Knowledge (>80% confidence)\n",
    "[What we can assert confidently]\n\n",
    "### Key Findings\n",
    "- [Finding] — [FACT/ESTIMATE/OPINION/UNKNOWN] — [confidence%]\n\n",
    "### Conflicts Resolved\n",
    "- [Conflict] → [Ruling and reasoning]\n\n",
    "### Genuine Uncertainties\n",
    "- [What requires more research]\n\n",
    "### Recommendations\n",
    "1. [Actionable recommendation]\n\n",
    "### What Would Change This Brief\n",
    "[Evidence that would overturn key findings]"
);

pub async fn run(config: &Config, topic: &str) -> Result<()> {
    let out_dir = research_dir(topic);
    std::fs::create_dir_all(&out_dir)?;

    eprintln!("\x1b[1m╔════════════════════════════════════════╗\x1b[0m");
    eprintln!("\x1b[1m║  CORTEX RESEARCH — LOCAL AI PIPELINE   ║\x1b[0m");
    eprintln!("\x1b[1m╚════════════════════════════════════════╝\x1b[0m");
    eprintln!("  Topic:  {topic}");
    eprintln!("  Output: {}", out_dir.display());
    eprintln!();

    ensure_daemon(config)?;

    // ── Phase 1: SCOUT (kimi-k2, agentic web search) + ORACLE (deepseek, reasoning) ──
    eprintln!("\x1b[33m[PHASE 1] SCOUT + ORACLE running in parallel...\x1b[0m");
    let scout_prompt = format!("{SCOUT_PROMPT}{topic}");
    let oracle_prompt = format!("{ORACLE_PROMPT}{topic}");

    let (scout_res, oracle_res) = tokio::join!(
        collect(config, &scout_prompt, ModelTier::KimiK2, "SCOUT", true),
        collect(
            config,
            &oracle_prompt,
            ModelTier::DeepSeekV31,
            "ORACLE",
            false
        ),
    );

    let scout = scout_res?;
    let oracle = oracle_res?;
    std::fs::write(out_dir.join("01-scout.md"), &scout)?;
    std::fs::write(out_dir.join("02-oracle.md"), &oracle)?;
    eprintln!("\x1b[32m[PHASE 1] Complete.\x1b[0m\n");

    // ── Phase 2: PHANTOM (adversarial) ──
    eprintln!("\x1b[33m[PHASE 2] PHANTOM adversarial review...\x1b[0m");
    let phantom_prompt = format!(
        "{PHANTOM_HEADER}### SCOUT OUTPUT:\n{scout}\n\n### ORACLE OUTPUT:\n{oracle}{PHANTOM_FOOTER}"
    );
    let phantom = collect(
        config,
        &phantom_prompt,
        ModelTier::Qwen3CoderNext,
        "PHANTOM",
        false,
    )
    .await?;
    std::fs::write(out_dir.join("03-phantom.md"), &phantom)?;
    eprintln!("\x1b[32m[PHASE 2] Complete.\x1b[0m\n");

    // ── Phase 3: VERDICT ──
    eprintln!("\x1b[33m[PHASE 3] VERDICT synthesis...\x1b[0m");
    let verdict_prompt = format!(
        "{VERDICT_HEADER}### SCOUT:\n{scout}\n\n### ORACLE:\n{oracle}\n\n### PHANTOM:\n{phantom}{VERDICT_FOOTER}"
    );
    let verdict = collect(
        config,
        &verdict_prompt,
        ModelTier::DeepSeekV31,
        "VERDICT",
        false,
    )
    .await?;
    std::fs::write(out_dir.join("04-verdict.md"), &verdict)?;

    eprintln!("\n\x1b[1m╔════════════════════════════════════════╗\x1b[0m");
    eprintln!("\x1b[1m║  INTELLIGENCE BRIEF DELIVERED          ║\x1b[0m");
    eprintln!("\x1b[1m╚════════════════════════════════════════╝\x1b[0m\n");
    println!("{verdict}");
    eprintln!(
        "\n\x1b[90mAll outputs saved to: {}\x1b[0m",
        out_dir.display()
    );

    Ok(())
}

async fn collect(
    config: &Config,
    prompt: &str,
    tier: ModelTier,
    role: &str,
    agentic: bool,
) -> Result<String> {
    let stream = UnixStream::connect(&config.daemon.socket_path)
        .await
        .map_err(|e| anyhow::anyhow!("cannot connect to daemon: {e}"))?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    let request = Request {
        id: nonce(),
        method: Method::Ask {
            prompt: prompt.to_string(),
            files: vec![],
            tier: Some(tier),
            cwd,
            agentic,
        },
    };

    let mut payload = serde_json::to_string(&request)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;

    let mut output = String::new();
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        let chunk: ResponseChunk = match serde_json::from_str(line.trim()) {
            Ok(c) => c,
            Err(_) => {
                line.clear();
                continue;
            }
        };
        match chunk {
            ResponseChunk::Token { text } => output.push_str(&text),
            ResponseChunk::Status { message } => {
                eprintln!("  \x1b[90m[{role}] {message}\x1b[0m");
            }
            ResponseChunk::Done {
                model_used,
                tokens_in,
                tokens_out,
                ..
            } => {
                eprintln!(
                    "  \x1b[90m[{role}] done — {model_used} {tokens_in}in/{tokens_out}out\x1b[0m"
                );
                break;
            }
            ResponseChunk::Error { message } => {
                anyhow::bail!("[{role}] model error: {message}");
            }
            _ => {}
        }
        line.clear();
    }

    Ok(output)
}

fn research_dir(topic: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let slug: String = topic
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    PathBuf::from(home)
        .join(".cortex")
        .join("research")
        .join(slug)
}

fn nonce() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(1)
}

fn ensure_daemon(config: &Config) -> Result<()> {
    if !config.daemon.socket_path.exists() {
        anyhow::bail!(
            "daemon not running — run: cortex start\n(or: cargo run --bin cortex-daemon)"
        );
    }
    Ok(())
}
