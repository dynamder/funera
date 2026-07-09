//! Skill system usage.
//!
//! ```bash
//! cargo run --example skills
//! ```
//!
//! Demonstrates:
//! - Defining a skill inline with `with_skill()`
//! - Activating a skill with `with_skill_active()`
//! - Loading SKILL.md files from disk with `with_skill_file()`
//! - Auto-discovering skills from `$SKILLS_HOME` / `~/.agents/skills/`
//! - How skill content is injected into the LLM's system prompt
//!
//! Skills are markdown files with YAML frontmatter that provide
//! reusable instructions, workflows, or constraints to the agent.

use funera_orchestrate::{Agent, AgentRuntime};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Runtime with skills ───────────────────────────────────────

    let runtime = AgentRuntime::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))

        // Inline skill: defines a reusable instruction fragment.
        // Skills are NOT activated by default — they need to be
        // explicitly activated (see `with_skill_active` below).
        .with_skill(
            "concise",
            "Prefer very short answers",
            "You must respond in 1-2 sentences maximum. Be direct and concise."
        )

        // Activate a skill by name. This causes its content to be
        // appended to the system prompt on every LLM call.
        .with_skill_active("concise")

        // Load a single SKILL.md file from disk.
        // SKILL.md files have YAML frontmatter:
        //   ---
        //   name: my-skill
        //   description: What it does
        //   ---
        //   ... markdown instructions ...
        // .with_skill_file("./path/to/my-skill.md")

        // Load all SKILL.md files from a directory.
        // .with_skills_dir("./skills/")

        // Auto-discover from $SKILLS_HOME env var,
        // falling back to ~/.agents/skills/.
        // Discovered skills are also auto-activated.
        // .with_skills_default_path()

        .build()?;

    // ── Agent ─────────────────────────────────────────────────────

    // The agent's system prompt is set first; active skills are
    // **appended after it** before each LLM call.
    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    // ── Query ─────────────────────────────────────────────────────

    // Because the "concise" skill is active, the model should
    // receive: [system prompt] + [skill content], so the
    // response should be noticeably short.
    let resp = agent
        .fire("What is the Rust programming language?", &runtime)
        .await?;

    println!("=== Response ===");
    println!("{}", resp.content);
    println!();
    println!("Iterations: {}", resp.iterations);

    // ── Runtime skill management ──────────────────────────────────
    //
    // Skills can also be managed at runtime through the env:
    //
    //   use funera_core::re_act::skills::Skill;
    //   let mut env = runtime.env();  // requires exposing env
    //   env.add_skill(Skill::new("debug", "Debug mode", "Explain your reasoning step by step.")).await;
    //   env.activate_skill("debug").await;

    Ok(())
}
