//! 4-phase dream consolidation prompt builder.
//!
//! Mirrors TS `consolidationPrompt.ts` exactly:
//! Phase 1 — Orient: scan memory dir, read MEMORY.md, skim existing files
//! Phase 2 — Gather: check daily logs, drifted memories, grep in transcripts
//! Phase 3 — Consolidate: merge new signals, resolve dates, delete contradicted facts
//! Phase 4 — Prune: update MEMORY.md index, remove stale pointers

/// The full auto-dream system prompt.
pub const AUTO_DREAM_SYSTEM_PROMPT: &str = r#"You are a memory consolidation agent running during idle time. Your job is to review, organize, and improve the user's persistent memory files.

You operate in 4 phases:

## Phase 1 — Orient
- List the memory directory contents
- Read the MEMORY.md index file
- Skim existing memory files to understand current state

## Phase 2 — Gather
- Check recent session transcripts for new information worth remembering
- Look for drifted or stale memories (outdated facts, broken references)
- Identify gaps and contradictions in existing memories

## Phase 3 — Consolidate
- Merge new signals from transcripts into existing memories
- Convert relative dates ("yesterday", "last week") to absolute dates
- Delete or update memories that have been contradicted by newer information
- Create new memories for important new facts, decisions, or patterns

## Phase 4 — Prune
- Update the MEMORY.md index to reflect all changes
- Ensure index entries stay under 150 characters each
- Remove stale pointers (files that no longer exist)
- Keep the total index under 200 lines
- Resolve any conflicts between overlapping memories

## Rules
- Only write files within the memory directory
- Use bash commands ONLY for reading (ls, find, grep, cat, stat, wc, head, tail)
- Never modify files outside the memory directory
- Preserve all valid existing content — only remove truly stale/contradicted facts
- Use absolute dates, never relative dates like "yesterday"
- Each memory file should have YAML frontmatter with name, description, and type fields
- Keep the MEMORY.md index concise: one line per entry, under 150 characters"#;

/// Build the user prompt for auto-dream consolidation.
pub fn build_dream_prompt(memory_dir: &str, session_dir: Option<&str>) -> String {
    let mut prompt = String::from("Begin memory consolidation.\n\n");

    prompt.push_str(&format!("Memory directory: {memory_dir}\n"));

    if let Some(sessions) = session_dir {
        prompt.push_str(&format!("Session transcripts directory: {sessions}\n"));
    }

    prompt.push_str(
        "\nStart with Phase 1 — Orient. List the memory directory, read MEMORY.md, \
         and understand the current state before making any changes.",
    );

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_is_nonempty() {
        assert!(!AUTO_DREAM_SYSTEM_PROMPT.is_empty());
        assert!(AUTO_DREAM_SYSTEM_PROMPT.contains("Phase 1"));
        assert!(AUTO_DREAM_SYSTEM_PROMPT.contains("Phase 4"));
    }

    #[test]
    fn dream_prompt_includes_paths() {
        let prompt = build_dream_prompt("/path/to/memory", Some("/path/to/sessions"));
        assert!(prompt.contains("/path/to/memory"));
        assert!(prompt.contains("/path/to/sessions"));
        assert!(prompt.contains("Phase 1"));
    }

    #[test]
    fn dream_prompt_works_without_session_dir() {
        let prompt = build_dream_prompt("/path/to/memory", None);
        assert!(prompt.contains("/path/to/memory"));
        assert!(!prompt.contains("Session transcripts"));
    }
}
