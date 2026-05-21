//! Compact prompt generation.
//!
//! Builds the system and user prompts used to ask the LLM to produce a
//! conversation summary.  Faithfully mirrors the prompt templates from
//! `services/compact/prompt.ts`.

// ---------------------------------------------------------------------------
// Prompt templates (kept as constants for zero-cost inclusion)
// ---------------------------------------------------------------------------

/// Aggressive no-tools preamble prepended to every compact prompt.
const NO_TOOLS_PREAMBLE: &str = "\
CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.

- Do NOT use Read, Bash, Grep, Glob, Edit, Write, or ANY other tool.
- You already have all the context you need in the conversation above.
- Tool calls will be REJECTED and will waste your only turn — you will fail the task.
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.

";

/// Trailer appended to every compact prompt.
const NO_TOOLS_TRAILER: &str = "\n\n\
REMINDER: Do NOT call any tools. Respond with plain text only — \
an <analysis> block followed by a <summary> block. \
Tool calls will be rejected and you will fail the task.";

/// Analysis instruction block used for full compaction.
const DETAILED_ANALYSIS_INSTRUCTION_BASE: &str = "\
Before providing your final summary, wrap your analysis in <analysis> tags \
to organize your thoughts and ensure you've covered all necessary points. \
In your analysis process:

1. Chronologically analyze each message and section of the conversation. \
For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, \
especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each \
required element thoroughly.";

/// Analysis instruction block used for partial compaction (recent messages).
const DETAILED_ANALYSIS_INSTRUCTION_PARTIAL: &str = "\
Before providing your final summary, wrap your analysis in <analysis> tags \
to organize your thoughts and ensure you've covered all necessary points. \
In your analysis process:

1. Analyze the recent messages chronologically. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, \
especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each \
required element thoroughly.";

/// Base compact prompt for full conversation summarization.
const BASE_COMPACT_PROMPT: &str = "\
Your task is to create a detailed summary of the conversation so far, paying \
close attention to the user's explicit requests and your previous actions.\n\
This summary should be thorough in capturing technical details, code patterns, \
and architectural decisions that would be essential for continuing development \
work without losing context.\n\n";

/// Summary sections template for full compaction.
const SUMMARY_SECTIONS: &str = "\
Your summary should include the following sections:

1. Primary Request and Intent: Capture all of the user's explicit requests and intents in detail
2. Key Technical Concepts: List all important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. \
Pay special attention to the most recent messages and include full code snippets where applicable and \
include a summary of why this file read or edit is important.
4. Errors and fixes: List all errors that you ran into, and how you fixed them. Pay special attention \
to specific user feedback that you received, especially if the user told you to do something differently.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages that are not tool results. These are critical for \
understanding the users' feedback and changing intent.
7. Pending Tasks: Outline any pending tasks that you have explicitly been asked to work on.
8. Current Work: Describe in detail precisely what was being worked on immediately before this summary \
request, paying special attention to the most recent messages from both user and assistant. Include file \
names and code snippets where applicable.
9. Optional Next Step: List the next step that you will take that is related to the most recent work \
you were doing. IMPORTANT: ensure that this step is DIRECTLY in line with the user's most recent explicit \
requests, and the task you were working on immediately before this summary request. If your last task was \
concluded, then only list next steps if they are explicitly in line with the users request. Do not start \
on tangential requests or really old requests that were already completed without confirming with the user first.
                       If there is a next step, include direct quotes from the most recent conversation \
showing exactly what task you were working on and where you left off. This should be verbatim to ensure \
there's no drift in task interpretation.\n\n\
Here's an example of how your output should be structured:\n\n\
<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]
   - [...]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Summary of the changes made to this file, if any]
      - [Important Code Snippet]
   - [File Name 2]
      - [Important Code Snippet]
   - [...]

4. Errors and fixes:
    - [Detailed description of error 1]:
      - [How you fixed the error]
      - [User feedback on the error if any]
    - [...]

5. Problem Solving:
   [Description of solved problems and ongoing troubleshooting]

6. All user messages:
    - [Detailed non tool use user message]
    - [...]

7. Pending Tasks:
   - [Task 1]
   - [Task 2]
   - [...]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>\n\n\
Please provide your summary based on the conversation so far, following this \
structure and ensuring precision and thoroughness in your response.\n\n\
There may be additional summarization instructions provided in the included \
context. If so, remember to follow these instructions when creating \
the above summary. Examples of instructions include:\n\
<example>
## Compact Instructions
When summarizing the conversation focus on typescript code changes and also \
remember the mistakes you made and how you fixed them.
</example>\n\n\
<example>
# Summary instructions
When you are using compact - please focus on test output and code changes. \
Include file reads verbatim.
</example>\n";

/// Partial compact prompt for summarizing recent messages (direction = "from").
const PARTIAL_COMPACT_PROMPT: &str = "\
Your task is to create a detailed summary of the RECENT portion of the \
conversation — the messages that follow earlier retained context. The earlier \
messages are being kept intact and do NOT need to be summarized. Focus your \
summary on what was discussed, learned, and accomplished in the recent \
messages only.\n\n";

/// Partial compact prompt for direction = "up_to".
const PARTIAL_COMPACT_UP_TO_PROMPT: &str = "\
Your task is to create a detailed summary of this conversation. This summary \
will be placed at the start of a continuing session; newer messages that build \
on this context will follow after your summary (you do not see them here). \
Summarize thoroughly so that someone reading only your summary and then the \
newer messages can fully understand what happened and continue the work.\n\n";

/// Summary sections for partial compaction — "from" direction.
const PARTIAL_SUMMARY_SECTIONS: &str = "\
Your summary should include the following sections:

1. Primary Request and Intent: Capture the user's explicit requests and intents from the recent messages
2. Key Technical Concepts: List important technical concepts, technologies, and frameworks discussed recently.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. \
Include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List errors encountered and how they were fixed.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages from the recent portion that are not tool results.
7. Pending Tasks: Outline any pending tasks from the recent messages.
8. Current Work: Describe precisely what was being worked on immediately before this summary request.
9. Optional Next Step: List the next step related to the most recent work. Include direct quotes from \
the most recent conversation.\n\n\
Here's an example of how your output should be structured:\n\n\
<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Important Code Snippet]

4. Errors and fixes:
    - [Error description]:
      - [How you fixed it]

5. Problem Solving:
   [Description]

6. All user messages:
    - [Detailed non tool use user message]

7. Pending Tasks:
    - [Task 1]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>\n\n\
Please provide your summary based on the RECENT messages only (after the retained earlier context), \
following this structure and ensuring precision and thoroughness in your response.\n";

/// Summary sections for partial compaction — "up_to" direction.
///
/// Uses different sections 7-9 ("Work Completed" / "Context for Continuing Work")
/// instead of "Current Work" / "Optional Next Step" because the summary is placed
/// at the start of a continuing session where newer messages follow.
const PARTIAL_SUMMARY_SECTIONS_UP_TO: &str = "\
Your summary should include the following sections:

1. Primary Request and Intent: Capture the user's explicit requests and intents in detail
2. Key Technical Concepts: List important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. \
Include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List errors encountered and how they were fixed.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages that are not tool results.
7. Pending Tasks: Outline any pending tasks.
8. Work Completed: Describe what was accomplished by the end of this portion.
9. Context for Continuing Work: Summarize any context, decisions, or state that would be needed to \
understand and continue the work in subsequent messages.\n\n\
Here's an example of how your output should be structured:\n\n\
<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Important Code Snippet]

4. Errors and fixes:
    - [Error description]:
      - [How you fixed it]

5. Problem Solving:
   [Description]

6. All user messages:
    - [Detailed non tool use user message]

7. Pending Tasks:
   - [Task 1]

8. Work Completed:
   [Description of what was accomplished]

9. Context for Continuing Work:
   [Key context, decisions, or state needed to continue the work]

</summary>
</example>\n\n\
Please provide your summary following this structure, ensuring precision and thoroughness in your response.\n";

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Direction for partial compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialCompactDirection {
    /// Summarize messages *after* the pivot, keep earlier ones.
    From,
    /// Summarize messages *before* the pivot, keep later ones.
    UpTo,
}

/// Build the full compact prompt (system prompt for the summarizer).
///
/// Mirrors `getCompactPrompt()` from the TypeScript reference.
pub fn build_compact_prompt(custom_instructions: Option<&str>) -> String {
    let mut prompt = format!(
        "{NO_TOOLS_PREAMBLE}{BASE_COMPACT_PROMPT}{DETAILED_ANALYSIS_INSTRUCTION_BASE}\n\n{SUMMARY_SECTIONS}"
    );

    if let Some(instructions) = custom_instructions {
        let trimmed = instructions.trim();
        if !trimmed.is_empty() {
            prompt.push_str("\n\nAdditional Instructions:\n");
            prompt.push_str(trimmed);
        }
    }

    prompt.push_str(NO_TOOLS_TRAILER);
    prompt
}

/// Build the partial compact prompt.
///
/// Mirrors `getPartialCompactPrompt()` from the TypeScript reference.
pub fn build_partial_compact_prompt(
    custom_instructions: Option<&str>,
    direction: PartialCompactDirection,
) -> String {
    let (template, analysis, sections) = match direction {
        PartialCompactDirection::From => (
            PARTIAL_COMPACT_PROMPT,
            DETAILED_ANALYSIS_INSTRUCTION_PARTIAL,
            PARTIAL_SUMMARY_SECTIONS,
        ),
        PartialCompactDirection::UpTo => (
            PARTIAL_COMPACT_UP_TO_PROMPT,
            DETAILED_ANALYSIS_INSTRUCTION_BASE,
            PARTIAL_SUMMARY_SECTIONS_UP_TO,
        ),
    };

    let mut prompt = format!("{NO_TOOLS_PREAMBLE}{template}{analysis}\n\n{sections}");

    if let Some(instructions) = custom_instructions {
        let trimmed = instructions.trim();
        if !trimmed.is_empty() {
            prompt.push_str("\n\nAdditional Instructions:\n");
            prompt.push_str(trimmed);
        }
    }

    prompt.push_str(NO_TOOLS_TRAILER);
    prompt
}

/// System prompt used for the compact summarizer LLM call.
pub const COMPACT_SYSTEM_PROMPT: &str =
    "You are a helpful AI assistant tasked with summarizing conversations.";

/// Format the raw LLM summary by stripping the `<analysis>` scratchpad and
/// converting `<summary>` tags into readable section headers.
///
/// Mirrors `formatCompactSummary()` from the TypeScript reference.
pub fn format_compact_summary(summary: &str) -> String {
    let mut formatted = summary.to_string();

    // Strip the analysis section — it's a drafting scratchpad.
    // The TS reference uses a non-global regex `.replace(/<analysis>[\s\S]*?<\/analysis>/, '')`
    // which removes only the first occurrence.
    if let Some(start) = formatted.find("<analysis>")
        && let Some(end) = formatted.find("</analysis>")
    {
        let analysis_end = end + "</analysis>".len();
        formatted = format!("{}{}", &formatted[..start], &formatted[analysis_end..]);
    }

    // Extract and format summary section
    if let Some(summary_start) = formatted.find("<summary>")
        && let Some(summary_end) = formatted.find("</summary>")
    {
        let content = &formatted[summary_start + "<summary>".len()..summary_end];
        let replacement = format!("Summary:\n{}", content.trim());
        formatted = format!(
            "{}{}{}",
            &formatted[..summary_start],
            replacement,
            &formatted[summary_end + "</summary>".len()..]
        );
    }

    // Clean up extra whitespace between sections
    while formatted.contains("\n\n\n") {
        formatted = formatted.replace("\n\n\n", "\n\n");
    }

    formatted.trim().to_string()
}

/// Build the user-facing summary message that wraps the formatted summary.
///
/// Mirrors `getCompactUserSummaryMessage()` from the TypeScript reference.
pub fn build_compact_user_summary_message(
    summary: &str,
    suppress_follow_up_questions: bool,
    transcript_path: Option<&str>,
    recent_messages_preserved: bool,
) -> String {
    build_compact_user_summary_message_ex(
        summary,
        suppress_follow_up_questions,
        transcript_path,
        recent_messages_preserved,
        false,
    )
}

/// Extended variant that supports proactive/autonomous mode.
///
/// When `proactive_active` is true and `suppress_follow_up_questions` is true,
/// appends the proactive continuation message matching the TS reference:
/// "You are running in autonomous/proactive mode. This is NOT a first
/// wake-up — you were already working autonomously before compaction..."
pub fn build_compact_user_summary_message_ex(
    summary: &str,
    suppress_follow_up_questions: bool,
    transcript_path: Option<&str>,
    recent_messages_preserved: bool,
    proactive_active: bool,
) -> String {
    let formatted = format_compact_summary(summary);

    let mut base = format!(
        "This session is being continued from a previous conversation that ran \
         out of context. The summary below covers the earlier portion of the \
         conversation.\n\n{formatted}"
    );

    if let Some(path) = transcript_path {
        base.push_str(&format!(
            "\n\nIf you need specific details from before compaction (like \
             exact code snippets, error messages, or content you generated), \
             read the full transcript at: {path}"
        ));
    }

    if recent_messages_preserved {
        base.push_str("\n\nRecent messages are preserved verbatim.");
    }

    if suppress_follow_up_questions {
        base.push_str(
            "\nContinue the conversation from where it left off without asking \
             the user any further questions. Resume directly — do not \
             acknowledge the summary, do not recap what was happening, do not \
             preface with \"I'll continue\" or similar. Pick up the last task \
             as if the break never happened.",
        );

        if proactive_active {
            base.push_str(
                "\n\nYou are running in autonomous/proactive mode. This is NOT \
                 a first wake-up — you were already working autonomously before \
                 compaction. Continue your work loop: pick up where you left off \
                 based on the summary above. Do not greet the user or ask what \
                 to work on.",
            );
        }
    }

    base
}

/// Rough token estimation using CJK/ASCII dual-ratio estimation.
///
/// Delegates to [`claude_provider::dual_ratio_estimate`] which classifies each
/// character as CJK (~1.5 chars/token) or ASCII (~4.0 chars/token) and
/// sums the estimates.  This is more accurate than the previous `len / 3`
/// heuristic, especially for mixed-language text.
pub fn rough_token_count(text: &str) -> u64 {
    claude_provider::dual_ratio_estimate(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_compact_summary_strips_analysis() {
        let input = "<analysis>thinking here</analysis>\n<summary>the summary</summary>";
        let result = format_compact_summary(input);
        assert!(!result.contains("<analysis>"));
        assert!(!result.contains("</analysis>"));
        assert!(result.contains("Summary:"));
        assert!(result.contains("the summary"));
    }

    #[test]
    fn format_compact_summary_cleans_whitespace() {
        let input = "<summary>content</summary>\n\n\n\nextra";
        let result = format_compact_summary(input);
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn build_compact_prompt_includes_custom_instructions() {
        let prompt = build_compact_prompt(Some("Focus on Rust code changes"));
        assert!(prompt.contains("Focus on Rust code changes"));
        assert!(prompt.contains("Additional Instructions:"));
    }

    #[test]
    fn build_compact_prompt_omits_empty_instructions() {
        let prompt_with = build_compact_prompt(Some(""));
        let prompt_without = build_compact_prompt(None);
        assert_eq!(prompt_with, prompt_without);
    }

    #[test]
    fn build_user_summary_message_includes_transcript_path() {
        let msg = build_compact_user_summary_message(
            "test summary",
            false,
            Some("/path/to/transcript"),
            false,
        );
        assert!(msg.contains("/path/to/transcript"));
    }

    #[test]
    fn rough_token_count_basic() {
        // "hello" = 5 chars → ceil(5/3) = 2 tokens
        assert_eq!(rough_token_count("hello"), 2);
    }
}
